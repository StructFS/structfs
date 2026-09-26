//! Process information store.

use std::collections::BTreeMap;

use structfs_core_store::{Error, NoCodec, Path, Reader, Record, Value, Writer};

/// The readable `self/*` entries, in listing order.
const SELF_ENTRIES: [&str; 5] = ["pid", "cwd", "args", "exe", "env"];

/// Store for information about the current process.
///
/// `proc/self/env` is the real process environment; writes through
/// [`EnvStore`](crate::EnvStore) live in that store's overlay and do not
/// appear here. Writing `proc/self/cwd` changes the process-wide working
/// directory.
pub struct ProcStore;

impl ProcStore {
    pub fn new() -> Self {
        Self
    }

    fn entry(name: &str) -> Result<Option<Value>, Error> {
        Ok(Some(match name {
            "pid" => Value::Integer(i64::from(std::process::id())),
            "cwd" => Value::String(std::env::current_dir()?.to_string_lossy().into_owned()),
            "args" => Value::Array(
                std::env::args_os()
                    .map(|a| Value::String(a.to_string_lossy().into_owned()))
                    .collect(),
            ),
            "exe" => Value::String(std::env::current_exe()?.to_string_lossy().into_owned()),
            "env" => Value::Map(
                std::env::vars_os()
                    .map(|(k, v)| {
                        (
                            k.to_string_lossy().into_owned(),
                            Value::String(v.to_string_lossy().into_owned()),
                        )
                    })
                    .collect(),
            ),
            _ => return Ok(None),
        }))
    }

    fn self_map() -> Result<Value, Error> {
        let mut map = BTreeMap::new();
        for name in SELF_ENTRIES {
            if let Some(value) = Self::entry(name)? {
                map.insert(name.to_string(), value);
            }
        }
        Ok(Value::Map(map))
    }

    fn read_value(path: &Path) -> Result<Option<Value>, Error> {
        match path.len() {
            0 => Ok(Some(Value::Map(BTreeMap::from([(
                "self".to_string(),
                Self::self_map()?,
            )])))),
            1 if &path[0] == "self" => Ok(Some(Self::self_map()?)),
            2 if &path[0] == "self" => Self::entry(&path[1]),
            _ => Ok(None),
        }
    }
}

impl Default for ProcStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for ProcStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        Ok(Self::read_value(from)?.map(Record::parsed))
    }
}

impl Writer for ProcStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        if to.len() != 2 || &to[0] != "self" || &to[1] != "cwd" {
            return Err(Error::permission_denied(format!(
                "proc/{to} is not writable; only proc/self/cwd accepts writes"
            )));
        }
        match data.into_value(&NoCodec)? {
            Value::String(dir) => std::env::set_current_dir(dir)?,
            _ => return Err(Error::invalid_argument("cwd must be a string path")),
        }
        Ok(to.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::path;

    fn read(at: &Path) -> Option<Value> {
        ProcStore::new()
            .read(at)
            .unwrap()
            .map(|r| r.into_value(&NoCodec).unwrap())
    }

    #[test]
    fn self_entries() {
        assert_eq!(
            read(&path!("self/pid")),
            Some(Value::Integer(std::process::id() as i64))
        );
        assert!(matches!(read(&path!("self/args")), Some(Value::Array(a)) if !a.is_empty()));
        assert!(matches!(read(&path!("self/cwd")), Some(Value::String(s)) if !s.is_empty()));
        assert!(matches!(read(&path!("self/exe")), Some(Value::String(s)) if !s.is_empty()));
        assert!(matches!(read(&path!("self/env")), Some(Value::Map(m)) if !m.is_empty()));
        assert_eq!(read(&path!("self/nonexistent")), None);
        assert_eq!(read(&path!("self/pid/extra")), None);
        assert_eq!(read(&path!("nonexistent")), None);
    }

    #[test]
    fn root_and_self_are_maps_of_values() {
        let Some(Value::Map(own)) = read(&path!("self")) else {
            panic!("expected map")
        };
        assert_eq!(own.len(), SELF_ENTRIES.len());
        assert_eq!(
            own.get("pid"),
            Some(&Value::Integer(std::process::id() as i64))
        );
        let Some(Value::Map(root)) = read(&path!("")) else {
            panic!("expected map")
        };
        assert!(matches!(root.get("self"), Some(Value::Map(m)) if m.contains_key("cwd")));
    }

    #[test]
    fn invalid_writes() {
        let mut store = ProcStore;
        for at in [path!("invalid"), path!("self/pid")] {
            assert!(matches!(
                store.write(&at, Record::parsed(Value::Integer(1))),
                Err(Error::PermissionDenied { .. })
            ));
        }
        assert!(matches!(
            store.write(&path!("self/cwd"), Record::parsed(Value::Integer(1))),
            Err(Error::InvalidArgument { .. })
        ));
        assert!(matches!(
            store.write(
                &path!("self/cwd"),
                Record::parsed(Value::String("/nonexistent/path/12345".into()))
            ),
            Err(Error::Io(_))
        ));
    }
}
