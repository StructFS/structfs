//! Documentation store for sys primitives (the docs protocol).
//!
//! Everything served here comes from [`SUBSYSTEMS`]; a test checks that the
//! crate README documents every listed operation verbatim.

use std::collections::BTreeMap;

use collection_literals::btree;
use structfs_core_store::{Error, Path, Reader, Record, Value, Writer};

/// One sub-store's documentation.
struct Subsystem {
    name: &'static str,
    title: &'static str,
    summary: &'static str,
    description: &'static str,
    /// `(operation, what it does)`; operations are written as REPL commands
    /// relative to the sys mount.
    paths: &'static [(&'static str, &'static str)],
}

const SUBSYSTEMS: &[Subsystem] = &[
    Subsystem {
        name: "env",
        title: "Environment Variables",
        summary: "Environment variables - read, set and unset in an overlay",
        description: "Reads see the process environment. Writes go to an in-process \
                      overlay private to this store; the process environment is never \
                      mutated, so child processes and proc/self/env do not see them.",
        paths: &[
            ("read env", "All variables (with the overlay applied) as a map"),
            ("read env/HOME", "One variable as a string; absent variables read as null"),
            ("write env/MY_VAR \"x\"", "Set a variable in the overlay"),
            ("write env/MY_VAR null", "Unset a variable in the overlay"),
        ],
    },
    Subsystem {
        name: "time",
        title: "Time Operations",
        summary: "Clocks and sleep - current time, monotonic, delays",
        description: "Wall-clock and monotonic clocks. Reading the root returns every \
                      clock's current value.",
        paths: &[
            ("read time/now", "Current time as an ISO 8601 / RFC 3339 string"),
            ("read time/now_unix", "Unix timestamp in seconds"),
            ("read time/now_unix_ms", "Unix timestamp in milliseconds"),
            ("read time/monotonic", "Nanoseconds since the first TimeStore was created"),
            ("write time/sleep {\"ms\": 100}", "Sleep; also accepts {\"secs\": N}; negative amounts and sleeps over one hour are rejected"),
        ],
    },
    Subsystem {
        name: "random",
        title: "Random Values",
        summary: "Random generation - integers, UUIDs, bytes",
        description: "Every read draws fresh values from the thread-local CSPRNG. The \
                      store is read-only.",
        paths: &[
            ("read random/u64", "A random unsigned 64-bit integer"),
            ("read random/uuid", "A random UUID v4 string"),
            ("read random/bytes/16", "N random bytes as a bytes value (at most 1 MiB)"),
        ],
    },
    Subsystem {
        name: "proc",
        title: "Process Information",
        summary: "Process info - PID, CWD, args, executable, environment",
        description: "Information about the current process. Reading proc or proc/self \
                      returns every entry's value.",
        paths: &[
            ("read proc/self/pid", "Process ID"),
            ("read proc/self/cwd", "Current working directory"),
            ("write proc/self/cwd \"/tmp\"", "Change the process-wide working directory"),
            ("read proc/self/args", "Command-line arguments"),
            ("read proc/self/exe", "Path to the executable"),
            ("read proc/self/env", "The real process environment"),
        ],
    },
    Subsystem {
        name: "fs",
        title: "Filesystem Operations",
        summary: "Filesystem - open, read, write, stat, readdir, mkdir, etc.",
        description: "Actions are writes of a request map. open returns handles/{id} for \
                      handle-based I/O; stat and readdir return results/{id} to read the \
                      answer from. An unrooted store reaches the whole filesystem; a \
                      rooted one refuses paths outside its root after symlink resolution.",
        paths: &[
            ("write fs/open {\"path\": \"/tmp/test.txt\", \"mode\": \"write\", \"encoding\": \"utf8\"}", "Open a file; returns handles/{id}"),
            ("read fs/handles", "Open handles, in id order"),
            ("read fs/handles/0", "Read from the cursor to EOF"),
            ("write fs/handles/0 \"Hello, World!\"", "Write at the cursor"),
            ("read fs/handles/0/at/10", "Read from byte 10 to EOF"),
            ("read fs/handles/0/at/10/len/4", "Read at most 4 bytes from byte 10"),
            ("write fs/handles/0/at/10 \"text\"", "Write from byte 10"),
            ("read fs/handles/0/position", "The cursor position"),
            ("write fs/handles/0/position {\"pos\": 0}", "Seek; negative positions are rejected"),
            ("read fs/handles/0/meta", "Size and type of the open file"),
            ("write fs/handles/0/close null", "Close the handle"),
            ("write fs/handles/0 null", "Also closes the handle (null deletes)"),
            ("write fs/stat {\"path\": \"/some/file\"}", "Stat a path; returns results/{id} holding size, kind, modified, readonly"),
            ("write fs/readdir {\"path\": \"/some/dir\"}", "List a directory; returns results/{id} holding sorted {name, kind} entries"),
            ("read fs/results/0", "Read a stat or readdir answer"),
            ("write fs/results/0 null", "Discard an answer"),
            ("write fs/mkdir {\"path\": \"/new/dir\", \"recursive\": true}", "Create a directory"),
            ("write fs/rmdir {\"path\": \"/dir\"}", "Remove an empty directory"),
            ("write fs/unlink {\"path\": \"/file\"}", "Remove a file (or a symlink itself)"),
            ("write fs/rename {\"from\": \"/old\", \"to\": \"/new\"}", "Rename a file or directory"),
            ("read fs/meta", "Machine-readable schemas for every action"),
        ],
    },
];

fn strings<'a>(items: impl IntoIterator<Item = &'a str>) -> Value {
    Value::Array(items.into_iter().map(|s| Value::String(s.into())).collect())
}

fn subsystem_docs(subsystem: &Subsystem) -> Value {
    Value::Map(btree! {
        "title".into() => Value::String(subsystem.title.into()),
        "description".into() => Value::String(subsystem.description.into()),
        "paths".into() => Value::Map(
            subsystem
                .paths
                .iter()
                .map(|(op, what)| (op.to_string(), Value::String(what.to_string())))
                .collect(),
        ),
    })
}

fn root_docs() -> Value {
    Value::Map(btree! {
        "title".into() => Value::String("System Primitives".into()),
        "description".into() => Value::String("OS primitives exposed through StructFS paths.".into()),
        "subsystems".into() => Value::Map(
            SUBSYSTEMS
                .iter()
                .map(|s| (s.name.to_string(), Value::String(s.summary.into())))
                .collect::<BTreeMap<_, _>>(),
        ),
        "examples".into() => strings(SUBSYSTEMS.iter().map(|s| s.paths[0].0)),
        "see_also".into() => Value::Array(
            SUBSYSTEMS
                .iter()
                .map(|s| Value::String(format!("docs/{}", s.name)))
                .collect(),
        ),
        "keywords".into() => strings(["sys", "env", "time", "random", "proc", "fs", "filesystem"]),
    })
}

/// Documentation store for sys primitives: `docs` is the overview and
/// `docs/{env,time,random,proc,fs}` document each sub-store.
pub struct DocsStore;

impl DocsStore {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DocsStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for DocsStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        let docs = match from.len() {
            0 => Some(root_docs()),
            1 => SUBSYSTEMS
                .iter()
                .find(|s| s.name == &from[0])
                .map(subsystem_docs),
            _ => None,
        };
        Ok(docs.map(Record::parsed))
    }
}

impl Writer for DocsStore {
    fn write(&mut self, _to: &Path, _data: Record) -> Result<Path, Error> {
        Err(Error::permission_denied("documentation is read-only"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::{path, NoCodec};

    fn read(at: &Path) -> Option<Value> {
        DocsStore::new()
            .read(at)
            .unwrap()
            .map(|r| r.into_value(&NoCodec).unwrap())
    }

    #[test]
    fn root_overview() {
        let Some(Value::Map(map)) = read(&path!("")) else {
            panic!("expected map")
        };
        assert_eq!(
            map.get("title"),
            Some(&Value::String("System Primitives".into()))
        );
        for key in ["description", "subsystems", "examples", "see_also"] {
            assert!(map.contains_key(key), "{key}");
        }
    }

    #[test]
    fn every_subsystem_has_real_docs() {
        for subsystem in SUBSYSTEMS {
            let at = Path::parse(subsystem.name).unwrap();
            let Some(Value::Map(map)) = read(&at) else {
                panic!("expected docs for {}", subsystem.name)
            };
            assert!(map.contains_key("title"));
            assert!(
                matches!(map.get("paths"), Some(Value::Map(paths)) if paths.len() == subsystem.paths.len())
            );
        }
    }

    #[test]
    fn unknown_and_nested_topics_are_absent() {
        assert_eq!(read(&path!("nonexistent")), None);
        assert_eq!(read(&path!("env/anything")), None);
    }

    #[test]
    fn writes_are_denied() {
        assert!(matches!(
            DocsStore.write(&path!("env"), Record::parsed(Value::Null)),
            Err(Error::PermissionDenied { .. })
        ));
    }

    #[test]
    fn readme_documents_every_operation() {
        let readme = include_str!("../README.md");
        for subsystem in SUBSYSTEMS {
            for (op, _) in subsystem.paths {
                assert!(
                    readme.contains(op),
                    "README.md does not document `{op}` ({})",
                    subsystem.name
                );
            }
        }
    }
}
