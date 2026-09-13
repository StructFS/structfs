//! Opt-in component-array representations. Default `Path` Serde remains a string.
//!
//! ```
//! #[derive(serde::Serialize, serde::Deserialize)]
//! struct Settings {
//!     #[serde(with = "structfs_core_store::path_serde::components")]
//!     path: structfs_core_store::Path,
//!     #[serde(default, with = "structfs_core_store::path_serde::optional_components")]
//!     parent: Option<structfs_core_store::Path>,
//! }
//! ```
//! Every array element is validated as one component: empty strings and strings
//! containing `/` are rejected, not normalized or split. `[]` is the root path.
use crate::Path;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub mod components {
    use super::*;
    pub fn serialize<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(path.iter())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Path, D::Error> {
        Path::try_from_components(Vec::<String>::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

pub mod optional_components {
    use super::*;
    pub fn serialize<S: Serializer>(path: &Option<Path>, serializer: S) -> Result<S::Ok, S::Error> {
        struct Components<'a>(&'a Path);
        impl Serialize for Components<'_> {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                components::serialize(self.0, s)
            }
        }
        match path {
            Some(path) => serializer.serialize_some(&Components(path)),
            None => serializer.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Path>, D::Error> {
        Option::<Vec<String>>::deserialize(deserializer)?
            .map(Path::try_from_components)
            .transpose()
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Record {
        #[serde(with = "components")]
        path: Path,
        #[serde(default, with = "optional_components")]
        parent: Option<Path>,
    }
    #[test]
    fn preserves_legacy_components_and_option_presence() {
        for json in [
            r#"{"path":["settings","accounts"],"parent":null}"#,
            r#"{"path":[],"parent":[]}"#,
        ] {
            let record: Record = serde_json::from_str(json).unwrap();
            assert_eq!(serde_json::to_string(&record).unwrap(), json);
        }
        assert_eq!(
            serde_json::to_string(&crate::path!("settings/accounts")).unwrap(),
            "\"settings/accounts\""
        );
        let missing: Record = serde_json::from_str(r#"{"path":[]}"#).unwrap();
        assert!(missing.parent.is_none());
    }
    #[test]
    fn rejects_invalid_components_in_both_adapters() {
        for component in ["", "a/b", "bad-name", ".", ".."] {
            for field in ["path", "parent"] {
                let mut record = serde_json::json!({"path":[], "parent":null});
                record[field] = serde_json::json!([component]);
                assert!(serde_json::from_value::<Record>(record).is_err());
            }
        }
    }
}
