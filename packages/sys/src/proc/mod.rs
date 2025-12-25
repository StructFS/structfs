//! Process information store.
//!
//! Provides access to process information.
//!
//! ## Paths
//!
//! - `proc/self/pid` - Read returns current process ID
//! - `proc/self/cwd` - Read returns current working directory, Write to chdir
//! - `proc/self/args` - Read returns command line arguments
//! - `proc/self/exe` - Read returns path to current executable
//! - `proc/self/env` - Read returns all environment variables

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value as JsonValue};
use structfs_store::{Error, Path, Reader, Writer};

/// Store for process information.
pub struct ProcStore;

impl ProcStore {
    pub fn new() -> Self {
        Self
    }

    fn read_value(&self, path: &Path) -> Result<Option<JsonValue>, Error> {
        if path.is_empty() {
            return Ok(Some(json!({
                "self": {
                    "pid": "Current process ID",
                    "cwd": "Current working directory",
                    "args": "Command line arguments",
                    "exe": "Path to current executable",
                    "env": "Environment variables"
                }
            })));
        }

        // Must start with "self"
        if path.components.first().map(|s| s.as_str()) != Some("self") {
            return Ok(None);
        }

        if path.components.len() == 1 {
            // Just /proc/self - list available info
            return Ok(Some(json!({
                "pid": "Current process ID",
                "cwd": "Current working directory",
                "args": "Command line arguments",
                "exe": "Path to current executable",
                "env": "Environment variables"
            })));
        }

        if path.components.len() != 2 {
            return Ok(None);
        }

        match path.components[1].as_str() {
            "pid" => Ok(Some(json!(std::process::id()))),
            "cwd" => match std::env::current_dir() {
                Ok(cwd) => Ok(Some(JsonValue::String(cwd.to_string_lossy().to_string()))),
                Err(e) => Err(Error::ImplementationFailure {
                    message: format!("Failed to get cwd: {}", e),
                }),
            },
            "args" => {
                let args: Vec<String> = std::env::args().collect();
                Ok(Some(json!(args)))
            }
            "exe" => match std::env::current_exe() {
                Ok(exe) => Ok(Some(JsonValue::String(exe.to_string_lossy().to_string()))),
                Err(e) => Err(Error::ImplementationFailure {
                    message: format!("Failed to get exe: {}", e),
                }),
            },
            "env" => {
                let vars: serde_json::Map<String, JsonValue> = std::env::vars()
                    .map(|(k, v)| (k, JsonValue::String(v)))
                    .collect();
                Ok(Some(JsonValue::Object(vars)))
            }
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
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, Error>
    where
        'this: 'de,
    {
        Ok(self.read_value(from)?.map(|v| {
            let de: Box<dyn erased_serde::Deserializer> =
                Box::new(<dyn erased_serde::Deserializer>::erase(v));
            de
        }))
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, Error> {
        if let Some(value) = self.read_value(from)? {
            let data: RecordType =
                serde_json::from_value(value).map_err(|err| Error::RecordDeserialization {
                    message: format!("Failed to deserialize proc value: {}", err),
                })?;
            Ok(Some(data))
        } else {
            Ok(None)
        }
    }
}

impl Writer for ProcStore {
    fn write<RecordType: Serialize>(
        &mut self,
        path: &Path,
        data: RecordType,
    ) -> Result<Path, Error> {
        // Must be proc/self/...
        if path.components.len() != 2 || path.components.first().map(|s| s.as_str()) != Some("self")
        {
            return Err(Error::ImplementationFailure {
                message: "Invalid proc path".to_string(),
            });
        }

        match path.components[1].as_str() {
            "cwd" => {
                let value =
                    serde_json::to_value(data).map_err(|err| Error::RecordSerialization {
                        message: format!("Failed to serialize cwd: {}", err),
                    })?;

                let new_cwd = value.as_str().ok_or_else(|| Error::ImplementationFailure {
                    message: "cwd must be a string path".to_string(),
                })?;

                std::env::set_current_dir(new_cwd).map_err(|e| Error::ImplementationFailure {
                    message: format!("Failed to change directory: {}", e),
                })?;

                Ok(path.clone())
            }
            _ => Err(Error::ImplementationFailure {
                message: format!("Cannot write to proc/self/{}", path.components[1]),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_pid() {
        let mut store = ProcStore::new();
        let result: u32 = store
            .read_owned(&Path::parse("self/pid").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(result, std::process::id());
    }

    #[test]
    fn test_read_cwd() {
        let mut store = ProcStore::new();
        let result: String = store
            .read_owned(&Path::parse("self/cwd").unwrap())
            .unwrap()
            .unwrap();
        assert!(!result.is_empty());
    }

    #[test]
    fn test_read_args() {
        let mut store = ProcStore::new();
        let result: Vec<String> = store
            .read_owned(&Path::parse("self/args").unwrap())
            .unwrap()
            .unwrap();
        assert!(!result.is_empty());
    }

    #[test]
    fn test_read_env() {
        let mut store = ProcStore::new();
        let result: serde_json::Map<String, JsonValue> = store
            .read_owned(&Path::parse("self/env").unwrap())
            .unwrap()
            .unwrap();
        assert!(!result.is_empty());
    }
}
