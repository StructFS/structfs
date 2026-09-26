//! Random value store.

use collection_literals::btree;
use rand::Rng;
use uuid::Uuid;

use structfs_core_store::{Error, Path, Reader, Record, Value, Writer};

/// The most bytes one `random/bytes/{n}` read may return (1 MiB).
pub const MAX_RANDOM_BYTES: u64 = 1024 * 1024;

/// Store for random values. Every read draws fresh values; the store is
/// read-only.
///
/// - `random/u64`: a random `u64` (`Value::Unsigned`)
/// - `random/uuid`: a random UUID v4 string
/// - `random/bytes/{n}`: `n` random bytes (`Value::Bytes`), `n` at most
///   [`MAX_RANDOM_BYTES`]
pub struct RandomStore;

impl RandomStore {
    pub fn new() -> Self {
        Self
    }

    fn bytes(count: &str) -> Result<Option<Value>, Error> {
        let Ok(count) = count.parse::<u64>() else {
            return Ok(None);
        };
        if count > MAX_RANDOM_BYTES {
            return Err(Error::resource_limit(format!(
                "random/bytes/{count} exceeds the {MAX_RANDOM_BYTES}-byte limit"
            )));
        }
        let mut bytes = vec![0u8; count as usize];
        rand::rng().fill(&mut bytes[..]);
        Ok(Some(Value::Bytes(bytes)))
    }

    fn read_value(path: &Path) -> Result<Option<Value>, Error> {
        let u64 = || Value::Unsigned(rand::rng().random());
        let uuid = || Value::String(Uuid::new_v4().to_string());
        Ok(match path.len() {
            // `bytes` is parameterized by its count, so it is not a child.
            0 => Some(Value::Map(btree! {
                "u64".into() => u64(),
                "uuid".into() => uuid(),
            })),
            1 => match &path[0] {
                "u64" => Some(u64()),
                "uuid" => Some(uuid()),
                _ => None,
            },
            2 if &path[0] == "bytes" => return Self::bytes(&path[1]),
            _ => None,
        })
    }
}

impl Default for RandomStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for RandomStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        Ok(Self::read_value(from)?.map(Record::parsed))
    }
}

impl Writer for RandomStore {
    fn write(&mut self, to: &Path, _data: Record) -> Result<Path, Error> {
        Err(Error::permission_denied(format!(
            "random/{to} is read-only; read random/bytes/{{n}} for random bytes"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::{path, NoCodec};

    fn read(at: &Path) -> Result<Option<Value>, Error> {
        Ok(RandomStore::new()
            .read(at)?
            .map(|r| r.into_value(&NoCodec).unwrap()))
    }

    #[test]
    fn uuid_and_u64() {
        let Some(Value::String(uuid)) = read(&path!("uuid")).unwrap() else {
            panic!("expected uuid string")
        };
        assert_eq!(uuid.len(), 36);
        assert_eq!(&uuid[14..15], "4");
        assert!(matches!(
            read(&path!("u64")).unwrap(),
            Some(Value::Unsigned(_))
        ));
        let Some(Value::Map(root)) = read(&path!("")).unwrap() else {
            panic!("expected map")
        };
        assert!(matches!(root.get("u64"), Some(Value::Unsigned(_))));
        assert!(matches!(root.get("uuid"), Some(Value::String(_))));
        assert_eq!(read(&path!("nonexistent")).unwrap(), None);
        assert_eq!(read(&path!("uuid/extra")).unwrap(), None);
    }

    #[test]
    fn bytes_are_readable_and_capped() {
        for n in [0usize, 3, 64] {
            let at = Path::parse(&format!("bytes/{n}")).unwrap();
            let Some(Value::Bytes(bytes)) = read(&at).unwrap() else {
                panic!("expected bytes")
            };
            assert_eq!(bytes.len(), n);
        }
        let max = Path::parse(&format!("bytes/{MAX_RANDOM_BYTES}")).unwrap();
        assert!(read(&max).unwrap().is_some());
        let over = Path::parse(&format!("bytes/{}", MAX_RANDOM_BYTES + 1)).unwrap();
        assert!(matches!(read(&over), Err(Error::ResourceLimit { .. })));
        assert_eq!(read(&path!("bytes")).unwrap(), None);
        assert_eq!(read(&path!("bytes/many")).unwrap(), None);
    }

    #[test]
    fn writes_are_denied() {
        let mut store = RandomStore;
        for at in [path!("bytes"), path!("u64"), path!("")] {
            assert!(matches!(
                store.write(&at, Record::parsed(Value::Null)),
                Err(Error::PermissionDenied { .. })
            ));
        }
    }
}
