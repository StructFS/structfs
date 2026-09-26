//! Time and clock store.

use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use structfs_core_store::{Error, NoCodec, Path, Reader, Record, Value, Writer};

/// The epoch of `time/monotonic`: first use of any `TimeStore`.
static MONOTONIC_START: LazyLock<Instant> = LazyLock::new(Instant::now);

/// The readable clocks, in listing order.
const CLOCKS: [&str; 4] = ["now", "now_unix", "now_unix_ms", "monotonic"];

/// Store for time operations.
pub struct TimeStore;

impl TimeStore {
    pub fn new() -> Self {
        LazyLock::force(&MONOTONIC_START);
        Self
    }

    fn clock(name: &str) -> Option<Value> {
        Some(match name {
            "now" => Value::String(chrono::Utc::now().to_rfc3339()),
            "now_unix" => Value::Integer(chrono::Utc::now().timestamp()),
            "now_unix_ms" => Value::Integer(chrono::Utc::now().timestamp_millis()),
            "monotonic" => Value::Unsigned(
                u64::try_from(MONOTONIC_START.elapsed().as_nanos()).unwrap_or(u64::MAX),
            ),
            _ => return None,
        })
    }

    fn read_value(path: &Path) -> Option<Value> {
        match path.len() {
            // The root is the map of every clock's current value; `sleep`
            // is write-only and so not a child.
            0 => Some(Value::Map(
                CLOCKS
                    .iter()
                    .filter_map(|name| Some((name.to_string(), Self::clock(name)?)))
                    .collect::<BTreeMap<_, _>>(),
            )),
            1 => Self::clock(&path[0]),
            _ => None,
        }
    }
}

impl Default for TimeStore {
    fn default() -> Self {
        Self::new()
    }
}

/// The longest sleep `time/sleep` accepts (one hour); longer requests are
/// `InvalidArgument`.
pub const MAX_SLEEP: Duration = Duration::from_secs(60 * 60);

/// A non-negative integer field of a sleep request.
fn amount(value: &Value, field: &str) -> Result<u64, Error> {
    match value {
        Value::Integer(n) => u64::try_from(*n)
            .map_err(|_| Error::invalid_argument(format!("sleep '{field}' must not be negative"))),
        Value::Unsigned(n) => Ok(*n),
        _ => Err(Error::invalid_argument(format!(
            "sleep '{field}' must be an integer"
        ))),
    }
}

impl Reader for TimeStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        Ok(Self::read_value(from).map(Record::parsed))
    }
}

impl Writer for TimeStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        if to.len() != 1 || &to[0] != "sleep" {
            return Err(Error::permission_denied(format!(
                "time/{to} is not writable; only time/sleep accepts writes"
            )));
        }
        let duration = match data.into_value(&NoCodec)? {
            Value::Map(map) => match (map.get("ms"), map.get("secs")) {
                (Some(ms), _) => Duration::from_millis(amount(ms, "ms")?),
                (None, Some(secs)) => Duration::from_secs(amount(secs, "secs")?),
                (None, None) => {
                    return Err(Error::invalid_argument(
                        "sleep requires an 'ms' or 'secs' field",
                    ))
                }
            },
            _ => {
                return Err(Error::invalid_argument(
                    "sleep requires a map with an 'ms' or 'secs' field",
                ))
            }
        };
        if duration > MAX_SLEEP {
            return Err(Error::invalid_argument(format!(
                "sleep of {duration:?} exceeds the {MAX_SLEEP:?} maximum"
            )));
        }
        std::thread::sleep(duration);
        Ok(to.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use collection_literals::btree;
    use structfs_core_store::path;

    fn read(at: &Path) -> Option<Value> {
        TimeStore::new()
            .read(at)
            .unwrap()
            .map(|r| r.into_value(&NoCodec).unwrap())
    }

    #[test]
    fn clocks() {
        assert!(matches!(read(&path!("now")), Some(Value::String(s)) if s.contains('T')));
        assert!(matches!(read(&path!("now_unix")), Some(Value::Integer(t)) if t > 0));
        assert!(matches!(read(&path!("now_unix_ms")), Some(Value::Integer(t)) if t > 0));
        let Some(Value::Unsigned(a)) = read(&path!("monotonic")) else {
            panic!("expected unsigned")
        };
        std::thread::sleep(Duration::from_millis(2));
        let Some(Value::Unsigned(b)) = read(&path!("monotonic")) else {
            panic!("expected unsigned")
        };
        assert!(b > a);
        assert_eq!(read(&path!("nonexistent")), None);
        assert_eq!(read(&path!("now/extra")), None);
    }

    #[test]
    fn root_is_a_map_of_clock_values() {
        let Some(Value::Map(root)) = read(&path!("")) else {
            panic!("expected map")
        };
        assert_eq!(root.keys().count(), CLOCKS.len());
        assert!(matches!(root.get("now"), Some(Value::String(s)) if s.contains('T')));
        assert!(matches!(root.get("now_unix"), Some(Value::Integer(_))));
        assert!(!root.contains_key("sleep"));
    }

    #[test]
    fn sleep() {
        let mut store = TimeStore::new();
        let before = Instant::now();
        store
            .write(
                &path!("sleep"),
                Record::parsed(Value::Map(btree! { "ms".into() => Value::Integer(1) })),
            )
            .unwrap();
        assert!(before.elapsed() >= Duration::from_millis(1));
        store
            .write(
                &path!("sleep"),
                Record::parsed(Value::Map(btree! { "secs".into() => Value::Unsigned(0) })),
            )
            .unwrap();
    }

    #[test]
    fn invalid_sleeps_are_rejected() {
        let mut store = TimeStore::new();
        for bad in [
            Value::Map(btree! { "ms".into() => Value::Integer(-1) }),
            Value::Map(btree! { "secs".into() => Value::Integer(-5) }),
            Value::Map(btree! { "ms".into() => Value::String("1".into()) }),
            Value::Map(btree! { "invalid".into() => Value::Integer(100) }),
            Value::String("100".into()),
            Value::Map(btree! { "secs".into() => Value::Integer(3601) }),
            Value::Map(btree! { "ms".into() => Value::Unsigned(u64::MAX) }),
        ] {
            let err = store
                .write(&path!("sleep"), Record::parsed(bad))
                .unwrap_err();
            assert!(matches!(err, Error::InvalidArgument { .. }), "{err}");
        }
        for bad in [path!("now"), path!("")] {
            assert!(matches!(
                store.write(&bad, Record::parsed(Value::Null)),
                Err(Error::PermissionDenied { .. })
            ));
        }
    }
}
