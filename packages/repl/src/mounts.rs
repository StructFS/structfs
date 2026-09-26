//! The REPL's mount configurations and the factory that builds them.
//!
//! `structfs-core-store`'s [`MountStore`](structfs_core_store::mount_store::MountStore)
//! owns only the mount mechanism; the set of store types a mount can name
//! lives here. A config is written as a map to `ctx/mounts/<name>`:
//!
//! ```text
//! write /ctx/mounts/data  {"type": "memory"}
//! write /ctx/mounts/state {"type": "local", "path": "/tmp/state.json"}
//! write /ctx/mounts/api   {"type": "http", "url": "https://api.example.com"}
//! ```

use serde::{Deserialize, Serialize};

use structfs_core_store::{
    mount_store::StoreFactory, overlay_store::StoreBox, Error, MemoryStore, Value,
};
use structfs_http::{BackgroundHttpBrokerStore, HttpBrokerStore, HttpClientStore};
use structfs_json_store::{BackedStore, JsonFileBacking, JsonlFileBacking, LogStore};
use structfs_sys::SysStore;

use crate::help_store::HelpStore;
use crate::recording_store::RecordingStore;
use crate::repl_docs_store::ReplDocsStore;
use crate::store_context::RegisterStore;

/// A mount configuration understood by [`CoreReplStoreFactory`].
///
/// The wire form is a map tagged by `type` (lowercase variant name), with
/// the variant's fields alongside: `{"type": "log", "path": "..."}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
#[non_exhaustive]
pub enum MountConfig {
    /// In-memory store (`MemoryStore`); contents are lost on exit.
    Memory,
    /// A JSON document on disk (`BackedStore` over `JsonFileBacking`):
    /// `path` is the file, created on first write and rewritten atomically
    /// after every write.
    Local { path: String },
    /// Direct HTTP client against a base URL (`HttpClientStore`): read is
    /// `GET <url>/<path>`, write is `POST`.
    Http { url: String },
    /// Synchronous HTTP broker (`HttpBrokerStore`): write a request, read
    /// its handle to execute it.
    HttpBroker,
    /// Background HTTP broker (`BackgroundHttpBrokerStore`): requests run
    /// on background threads. `"asynchttpbroker"` is accepted as the
    /// pre-0.5 name.
    #[serde(alias = "asynchttpbroker")]
    BackgroundHttpBroker,
    /// Help/documentation store (read-only).
    Help,
    /// System primitives (env, time, proc, fs, random) with unconfined
    /// filesystem access; see `structfs_sys::SysStore::new`.
    Sys,
    /// REPL documentation store.
    Repl,
    /// Register storage (session-local named values).
    Registers,
    /// A JSONL append log served as a store (ledgers, transcripts, session
    /// logs): read `/` for every entry, `len` for the count, `entries/{n}`
    /// for one, and page with the `entries/from/{n}` cursor tail. Writing
    /// `append` adds an entry.
    Log { path: String },
    /// A recording directory (as `fw run --record DIR` writes it) as one
    /// read-only tree: the session log at `session`, each block's
    /// transcript at its assembly-scoped key.
    Recording { path: String },
}

impl MountConfig {
    /// Every mount type with a one-line description, for docs and help.
    pub const TYPES: &'static [(&'static str, &'static str)] = &[
        ("memory", "In-memory store (lost on exit)"),
        (
            "local",
            "A JSON document on disk; {\"path\": FILE}, saved after every write",
        ),
        ("http", "HTTP client for a base URL; {\"url\": URL}"),
        ("httpbroker", "Sync HTTP request broker"),
        (
            "backgroundhttpbroker",
            "Background HTTP request broker (alias: asynchttpbroker)",
        ),
        ("help", "The help/documentation store"),
        ("sys", "OS primitives (env, time, proc, fs, random)"),
        ("repl", "The REPL's own documentation"),
        ("registers", "Session-local named values"),
        (
            "log",
            "A JSONL append log (ledger, transcript, session log); page with entries/from/{n}",
        ),
        (
            "recording",
            "A recorded run's directory, read-only: session timeline plus per-block transcripts",
        ),
    ];

    /// Decode a config from the value written to `ctx/mounts/<name>`.
    /// Malformed configs are `Error::InvalidArgument`.
    pub fn from_value(value: Value) -> Result<Self, Error> {
        if !matches!(value, Value::Map(_)) {
            return Err(Error::invalid_argument("mount config must be a map"));
        }
        structfs_serde_store::from_value(value)
            .map_err(|e| Error::invalid_argument(format!("invalid mount config: {e}")))
    }

    /// Encode a config in its wire form.
    pub fn to_value(&self) -> Result<Value, Error> {
        structfs_serde_store::to_value(self)
    }
}

/// A default mount: where the REPL mounts it at startup and what it is.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct DefaultMount {
    /// Mount name (path without the leading `/`).
    pub name: &'static str,
    /// The store mounted there.
    pub kind: DefaultMountKind,
    /// One-line description for help and the README.
    pub summary: &'static str,
}

/// How a [`DefaultMount`] is created.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum DefaultMountKind {
    /// Created by the factory from this config.
    Config(fn() -> MountConfig),
    /// The help store, which shares state with the context and is mounted
    /// last so it indexes every other mount's docs.
    Help,
}

/// The mounts every REPL session starts with, in mount order. This is the
/// single source for the startup code, `help`, and the README table.
pub const DEFAULT_MOUNTS: &[DefaultMount] = &[
    DefaultMount {
        name: "ctx/repl",
        kind: DefaultMountKind::Config(|| MountConfig::Repl),
        summary: "REPL documentation",
    },
    DefaultMount {
        name: "ctx/http",
        kind: DefaultMountKind::Config(|| MountConfig::BackgroundHttpBroker),
        summary: "HTTP broker (background threads)",
    },
    DefaultMount {
        name: "ctx/http_sync",
        kind: DefaultMountKind::Config(|| MountConfig::HttpBroker),
        summary: "HTTP broker (sync, blocks on read)",
    },
    DefaultMount {
        name: "ctx/sys",
        kind: DefaultMountKind::Config(|| MountConfig::Sys),
        summary: "System primitives (env, time, random, proc, fs); unconfined filesystem access",
    },
    DefaultMount {
        name: "ctx/registers",
        kind: DefaultMountKind::Config(|| MountConfig::Registers),
        summary: "Registers (session-local named values)",
    },
    DefaultMount {
        name: "ctx/help",
        kind: DefaultMountKind::Help,
        summary: "Documentation system",
    },
];

/// Factory for creating stores from [`MountConfig`]s.
///
/// This is the default factory used by `StoreContext`.
pub struct CoreReplStoreFactory;

fn create_error(what: &str, e: impl std::fmt::Display) -> Error {
    Error::store("factory", "create", format!("failed to create {what}: {e}"))
}

impl StoreFactory for CoreReplStoreFactory {
    type Config = MountConfig;

    fn create(&self, config: &MountConfig) -> Result<StoreBox, Error> {
        Ok(match config {
            MountConfig::Memory => Box::new(MemoryStore::new()),
            MountConfig::Local { path } => Box::new(BackedStore::open(JsonFileBacking::new(path))?),
            MountConfig::Http { url } => Box::new(HttpClientStore::new(url).map_err(Error::from)?),
            MountConfig::HttpBroker => Box::new(
                HttpBrokerStore::with_default_timeout()
                    .map_err(|e| create_error("HTTP broker", e))?,
            ),
            MountConfig::BackgroundHttpBroker => Box::new(
                BackgroundHttpBrokerStore::with_default_timeout()
                    .map_err(|e| create_error("background HTTP broker", e))?,
            ),
            MountConfig::Help => Box::new(HelpStore::new()),
            MountConfig::Sys => Box::new(SysStore::new()),
            MountConfig::Repl => Box::new(ReplDocsStore::new()),
            MountConfig::Registers => Box::new(RegisterStore::new()),
            MountConfig::Log { path } => Box::new(LogStore::open(JsonlFileBacking::new(path))?),
            MountConfig::Recording { path } => Box::new(RecordingStore::open(path)?),
        })
    }

    fn config_from_value(&self, value: Value) -> Result<MountConfig, Error> {
        MountConfig::from_value(value)
    }

    fn config_to_value(&self, config: &MountConfig) -> Result<Value, Error> {
        config.to_value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use collection_literals::btree;
    use structfs_core_store::{path, Reader, Record, Writer};

    fn tagged(fields: &[(&str, &str)]) -> Value {
        Value::Map(
            fields
                .iter()
                .map(|(k, v)| (k.to_string(), Value::from(*v)))
                .collect(),
        )
    }

    /// The pre-0.5 wire form of every config that still exists, as core-store's
    /// hand-rolled converter wrote it. These must be byte-for-byte stable.
    fn legacy_wire_forms() -> Vec<(Value, MountConfig)> {
        vec![
            (tagged(&[("type", "memory")]), MountConfig::Memory),
            (
                tagged(&[("type", "local"), ("path", "/tmp/test")]),
                MountConfig::Local {
                    path: "/tmp/test".into(),
                },
            ),
            (
                tagged(&[("type", "http"), ("url", "https://api.example.com")]),
                MountConfig::Http {
                    url: "https://api.example.com".into(),
                },
            ),
            (tagged(&[("type", "httpbroker")]), MountConfig::HttpBroker),
            (tagged(&[("type", "help")]), MountConfig::Help),
            (tagged(&[("type", "sys")]), MountConfig::Sys),
            (tagged(&[("type", "repl")]), MountConfig::Repl),
            (tagged(&[("type", "registers")]), MountConfig::Registers),
            (
                tagged(&[("type", "log"), ("path", "/tmp/log.jsonl")]),
                MountConfig::Log {
                    path: "/tmp/log.jsonl".into(),
                },
            ),
            (
                tagged(&[("type", "recording"), ("path", "/tmp/rec")]),
                MountConfig::Recording {
                    path: "/tmp/rec".into(),
                },
            ),
        ]
    }

    #[test]
    fn wire_form_is_unchanged_for_existing_configs() {
        for (wire, config) in legacy_wire_forms() {
            assert_eq!(MountConfig::from_value(wire.clone()).unwrap(), config);
            assert_eq!(config.to_value().unwrap(), wire, "{config:?}");
        }
    }

    #[test]
    fn background_broker_accepts_the_old_tag_and_lists_the_new_one() {
        let old = MountConfig::from_value(tagged(&[("type", "asynchttpbroker")])).unwrap();
        let new = MountConfig::from_value(tagged(&[("type", "backgroundhttpbroker")])).unwrap();
        assert_eq!(old, MountConfig::BackgroundHttpBroker);
        assert_eq!(new, MountConfig::BackgroundHttpBroker);
        assert_eq!(
            old.to_value().unwrap(),
            tagged(&[("type", "backgroundhttpbroker")])
        );
    }

    #[test]
    fn types_table_names_every_variant() {
        let names: Vec<&str> = MountConfig::TYPES.iter().map(|(n, _)| *n).collect();
        for (wire, _) in legacy_wire_forms() {
            let Value::Map(map) = wire else { panic!() };
            let Some(Value::String(t)) = map.get("type") else {
                panic!()
            };
            assert!(names.contains(&t.as_str()), "{t} missing from TYPES");
        }
        assert!(names.contains(&"backgroundhttpbroker"));
    }

    #[test]
    fn malformed_configs_are_invalid_argument() {
        for bad in [
            Value::from("not a map"),
            Value::Map(Default::default()),
            tagged(&[("type", "unknown")]),
            tagged(&[("type", "structfs"), ("url", "https://x")]),
            tagged(&[("type", "local")]),
            tagged(&[("type", "http")]),
            tagged(&[("type", "log")]),
            tagged(&[("type", "recording")]),
            Value::Map(btree! {"type".to_string() => Value::Integer(1)}),
        ] {
            let err = MountConfig::from_value(bad.clone()).unwrap_err();
            assert!(
                matches!(err, Error::InvalidArgument { .. }),
                "{bad:?}: {err}"
            );
        }
    }

    #[test]
    fn local_mount_persists_a_json_document() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("state.json");
        let config = MountConfig::Local {
            path: file.to_string_lossy().into_owned(),
        };

        let mut store = CoreReplStoreFactory.create(&config).unwrap();
        store
            .write(&path!("users/alice"), Record::parsed(Value::from("Alice")))
            .unwrap();
        drop(store);
        assert!(file.exists());

        let mut reopened = CoreReplStoreFactory.create(&config).unwrap();
        assert_eq!(
            reopened
                .read(&path!("users/alice"))
                .unwrap()
                .unwrap()
                .as_value(),
            Some(&Value::from("Alice"))
        );
    }

    #[test]
    fn http_mount_builds_a_client_store() {
        assert!(CoreReplStoreFactory
            .create(&MountConfig::Http {
                url: "https://api.example.com".into(),
            })
            .is_ok());
        let err = CoreReplStoreFactory
            .create(&MountConfig::Http {
                url: "not a url".into(),
            })
            .err()
            .expect("an unparseable base URL is refused");
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn factory_creates_every_config_free_store() {
        for config in [
            MountConfig::Memory,
            MountConfig::HttpBroker,
            MountConfig::BackgroundHttpBroker,
            MountConfig::Help,
            MountConfig::Sys,
            MountConfig::Repl,
            MountConfig::Registers,
        ] {
            assert!(CoreReplStoreFactory.create(&config).is_ok(), "{config:?}");
        }
    }

    #[test]
    fn default_mount_names_are_unique_and_help_is_last() {
        let mut names: Vec<&str> = DEFAULT_MOUNTS.iter().map(|m| m.name).collect();
        assert!(matches!(
            DEFAULT_MOUNTS.last().unwrap().kind,
            DefaultMountKind::Help
        ));
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), DEFAULT_MOUNTS.len());
    }

    /// The README's "Default Mounts" table is generated from
    /// [`DEFAULT_MOUNTS`]; this fails when the two drift.
    #[test]
    fn readme_default_mount_table_matches_the_code() {
        let readme = include_str!("../README.md");
        let expected: String = DEFAULT_MOUNTS
            .iter()
            .map(|m| format!("| `/{}` | {} |\n", m.name, m.summary))
            .collect();
        assert!(
            readme.contains(&expected),
            "README default-mount table is stale; it should contain:\n{expected}"
        );
    }
}
