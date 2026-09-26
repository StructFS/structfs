use std::collections::BTreeMap;

use structfs_core_store::{path, Error, NoCodec, Path, Reader, Record, Value, Writer};
use tempfile::TempDir;

use super::handles::{parse_handle_operation, HandleOperation};
use super::*;

/// A request map from `(field, value)` pairs.
fn req(fields: &[(&str, Value)]) -> Value {
    Value::Map(
        fields
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect::<BTreeMap<_, _>>(),
    )
}

fn s(text: &str) -> Value {
    Value::String(text.to_string())
}

fn os(path: &std::path::Path) -> Value {
    s(&path.to_string_lossy())
}

fn read(store: &mut FsStore, at: &Path) -> Option<Value> {
    store
        .read(at)
        .unwrap()
        .map(|r| r.into_value(&NoCodec).unwrap())
}

fn write(store: &mut FsStore, at: &Path, value: Value) -> Result<Path, Error> {
    store.write(at, Record::parsed(value))
}

/// Open `file` with `mode` and optional `encoding`; returns the handle path.
fn open(store: &mut FsStore, file: Value, mode: &str, encoding: Option<&str>) -> Path {
    let mut fields = vec![("path", file), ("mode", s(mode))];
    if let Some(encoding) = encoding {
        fields.push(("encoding", s(encoding)));
    }
    write(store, &path!("open"), req(&fields)).unwrap()
}

/// A temp dir holding `name` with `content`.
fn fixture(name: &str, content: &str) -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join(name);
    std::fs::write(&file, content).unwrap();
    (dir, file)
}

fn decode_b64(value: Value) -> Vec<u8> {
    let Value::String(s) = value else {
        panic!("expected base64 string, got {value:?}")
    };
    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s).unwrap()
}

// ---- handles ------------------------------------------------------------

#[test]
fn open_and_read_base64_by_default() {
    let (_dir, file) = fixture("a.txt", "Hello, world!");
    let mut store = FsStore::new();
    let handle = write(&mut store, &path!("open"), req(&[("path", os(&file))])).unwrap();
    assert_eq!(handle, path!("handles/0"));
    let content = decode_b64(read(&mut store, &handle).unwrap());
    assert_eq!(content, b"Hello, world!");
}

#[test]
fn write_utf8_and_bytes_then_close() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("out.txt");
    let mut store = FsStore::new();
    let handle = open(&mut store, os(&file), "write", Some("utf8"));
    write(&mut store, &handle, s("text ")).unwrap();
    write(&mut store, &handle, Value::Bytes(b"bytes".to_vec())).unwrap();
    write(&mut store, &handle.join(&path!("close")), Value::Null).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "text bytes");
    assert!(matches!(
        write(&mut store, &handle.join(&path!("close")), Value::Null),
        Err(Error::NotFound { .. })
    ));
}

#[test]
fn append_readwrite_and_create_new_modes() {
    let (dir, file) = fixture("f.txt", "first");
    let mut store = FsStore::new();
    let handle = open(&mut store, os(&file), "append", Some("text"));
    write(&mut store, &handle, s("second")).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "firstsecond");

    open(
        &mut store,
        os(&dir.path().join("rw.txt")),
        "readwrite",
        None,
    );
    open(&mut store, os(&dir.path().join("n1")), "create_new", None);
    open(&mut store, os(&dir.path().join("n2")), "createnew", None);
    let exists = write(
        &mut store,
        &path!("open"),
        req(&[("path", os(&file)), ("mode", s("create_new"))]),
    );
    assert!(matches!(exists, Err(Error::Io(_))));
}

#[test]
fn unknown_mode_or_encoding_is_invalid_argument() {
    let (_dir, file) = fixture("f.txt", "x");
    let mut store = FsStore::new();
    for fields in [
        vec![("path", os(&file)), ("mode", s("sideways"))],
        vec![("path", os(&file)), ("encoding", s("latin1"))],
        vec![("path", os(&file)), ("mode", Value::Integer(1))],
        vec![("mode", s("read"))],
    ] {
        let err = write(&mut store, &path!("open"), req(&fields)).unwrap_err();
        assert!(matches!(err, Error::InvalidArgument { .. }), "{err}");
    }
    assert!(store.handles.is_empty());
    let err = write(&mut store, &path!("open"), s("not a map")).unwrap_err();
    assert!(matches!(err, Error::InvalidArgument { .. }));
}

#[test]
fn open_missing_file_is_io_error() {
    let mut store = FsStore::new();
    let err = write(
        &mut store,
        &path!("open"),
        req(&[("path", s("/nonexistent/path/12345"))]),
    )
    .unwrap_err();
    assert!(matches!(err, Error::Io(_)));
}

#[test]
fn offsets_lengths_and_position() {
    let (_dir, file) = fixture("digits", "0123456789");
    let mut store = FsStore::new();
    let handle = open(&mut store, os(&file), "readwrite", Some("utf8"));
    assert_eq!(
        read(&mut store, &handle.join(&path!("at/5"))),
        Some(s("56789"))
    );
    assert_eq!(
        read(&mut store, &handle.join(&path!("at/2/len/3"))),
        Some(s("234"))
    );
    let position = handle.join(&path!("position"));
    assert_eq!(
        read(&mut store, &position),
        Some(req(&[("position", Value::Integer(5))]))
    );

    write(&mut store, &handle.join(&path!("at/3")), s("XXX")).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "012XXX6789");

    write(&mut store, &position, req(&[("pos", Value::Integer(1))])).unwrap();
    assert_eq!(
        read(&mut store, &handle.join(&path!("at/1/len/2"))),
        Some(s("12"))
    );
    write(&mut store, &position, req(&[("pos", Value::Unsigned(8))])).unwrap();
    assert_eq!(read(&mut store, &handle), Some(s("89")));
}

#[test]
fn position_rejects_negative_and_malformed() {
    let (_dir, file) = fixture("f", "content");
    let mut store = FsStore::new();
    let handle = open(&mut store, os(&file), "read", None);
    let position = handle.join(&path!("position"));
    for bad in [
        req(&[("pos", Value::Integer(-1))]),
        req(&[]),
        s("5"),
        req(&[("pos", s("5"))]),
    ] {
        let err = write(&mut store, &position, bad).unwrap_err();
        assert!(matches!(err, Error::InvalidArgument { .. }), "{err}");
    }
    let err = write(
        &mut store,
        &path!("meta/handles/0/position"),
        Value::Integer(-7),
    )
    .unwrap_err();
    assert!(matches!(err, Error::InvalidArgument { .. }), "{err}");
}

#[test]
fn read_len_is_capped() {
    let (_dir, file) = fixture("big", "0123456789");
    let mut store = FsStore::new().with_max_read_len(4);
    let handle = open(&mut store, os(&file), "read", Some("utf8"));
    // The requested length is checked before anything is allocated.
    let huge = handle.join(&Path::parse("at/0/len/18446744073709551615").unwrap());
    assert!(matches!(
        store.read(&huge),
        Err(Error::ResourceLimit { .. })
    ));
    assert!(matches!(
        store.read(&handle),
        Err(Error::ResourceLimit { .. })
    ));
    assert_eq!(
        read(&mut store, &handle.join(&path!("at/0/len/4"))),
        Some(s("0123"))
    );
    assert_eq!(
        read(&mut store, &handle.join(&path!("at/6"))),
        Some(s("6789"))
    );
}

#[test]
fn encodings() {
    let (_dir, file) = fixture("enc", "raw bytes");
    let mut store = FsStore::new();
    let bytes = open(&mut store, os(&file), "read", Some("bytes"));
    assert_eq!(
        read(&mut store, &bytes),
        Some(Value::Bytes(b"raw bytes".to_vec()))
    );
    let raw = open(&mut store, os(&file), "read", Some("RAW"));
    assert_eq!(
        read(&mut store, &raw),
        Some(Value::Bytes(b"raw bytes".to_vec()))
    );

    let (_dir2, binary) = fixture("bin", "");
    std::fs::write(&binary, [0xff, 0xfe]).unwrap();
    let text = open(&mut store, os(&binary), "read", Some("utf-8"));
    assert!(matches!(
        store.read(&text),
        Err(Error::InvalidArgument { .. })
    ));

    let b64 = open(&mut store, os(&binary), "write", None);
    assert!(matches!(
        write(&mut store, &b64, s("not base64!")),
        Err(Error::InvalidArgument { .. })
    ));
    assert!(matches!(
        write(&mut store, &b64, Value::Integer(1)),
        Err(Error::InvalidArgument { .. })
    ));
}

#[test]
fn handle_meta_and_listing_are_ordered() {
    let (_dir, file) = fixture("m", "content");
    let mut store = FsStore::new();
    for _ in 0..12 {
        open(&mut store, os(&file), "read", None);
    }
    let Some(Value::Map(listing)) = read(&mut store, &path!("handles")) else {
        panic!("expected listing")
    };
    let Some(Value::Array(items)) = listing.get("items") else {
        panic!("expected items")
    };
    let paths: Vec<_> = items
        .iter()
        .map(|item| item.get(&path!("path")).cloned().unwrap())
        .collect();
    let expected: Vec<_> = (0..12).map(|i| s(&format!("handles/{i}"))).collect();
    assert_eq!(paths, expected);

    let Some(Value::Map(meta)) = read(&mut store, &path!("handles/3/meta")) else {
        panic!("expected meta")
    };
    assert_eq!(meta.get("size"), Some(&Value::Integer(7)));
    assert_eq!(meta.get("is_file"), Some(&Value::Bool(true)));
}

#[test]
fn handle_ids_are_per_store() {
    let (_dir, file) = fixture("f", "x");
    let mut a = FsStore::new();
    let mut b = FsStore::new();
    assert_eq!(open(&mut a, os(&file), "read", None), path!("handles/0"));
    assert_eq!(open(&mut b, os(&file), "read", None), path!("handles/0"));
    assert_eq!(open(&mut a, os(&file), "read", None), path!("handles/1"));
}

#[test]
fn handle_path_errors() {
    let (_dir, file) = fixture("f", "x");
    let mut store = FsStore::new();
    let handle = open(&mut store, os(&file), "read", None);
    assert!(matches!(
        store.read(&path!("handles/invalid")),
        Err(Error::InvalidArgument { .. })
    ));
    assert!(matches!(
        store.read(&path!("handles/999")),
        Err(Error::NotFound { .. })
    ));
    assert!(matches!(
        store.read(&handle.join(&path!("close"))),
        Err(Error::PermissionDenied { .. })
    ));
    assert!(matches!(
        store.read(&handle.join(&path!("bogus"))),
        Err(Error::InvalidArgument { .. })
    ));
    for read_only in [path!("meta"), path!("at/0/len/1")] {
        assert!(matches!(
            write(&mut store, &handle.join(&read_only), s("x")),
            Err(Error::PermissionDenied { .. })
        ));
    }
}

#[test]
fn parse_handle_operation_forms() {
    assert!(parse_handle_operation(&path!("")).is_none());
    assert!(parse_handle_operation(&path!("other")).is_none());
    assert!(parse_handle_operation(&path!("handles/abc")).is_none());
    let cases = [
        (path!("handles/42"), HandleOperation::Cursor),
        (path!("handles/5/meta"), HandleOperation::Meta),
        (path!("handles/5/close"), HandleOperation::Close),
        (path!("handles/5/position"), HandleOperation::Position),
        (
            path!("handles/5/at/100"),
            HandleOperation::AtOffset { offset: 100 },
        ),
        (
            path!("handles/5/at/10/len/20"),
            HandleOperation::ReadAtLen {
                offset: 10,
                length: 20,
            },
        ),
    ];
    for (path, op) in cases {
        assert_eq!(parse_handle_operation(&path).unwrap().1, op, "{path}");
    }
    for bad in [
        path!("handles/5/unknown"),
        path!("handles/5/at/abc"),
        path!("handles/5/at/10/len/abc"),
        path!("handles/5/at"),
    ] {
        assert!(parse_handle_operation(&bad).is_none(), "{bad}");
    }
}

// ---- path actions -------------------------------------------------------

#[test]
fn stat_reports_size_kind_modified_readonly() {
    let (dir, file) = fixture("s.txt", "12345");
    let mut store = FsStore::new();
    let result = write(&mut store, &path!("stat"), req(&[("path", os(&file))])).unwrap();
    assert_eq!(result, path!("results/0"));
    let Some(Value::Map(stat)) = read(&mut store, &result) else {
        panic!("expected stat map")
    };
    assert_eq!(stat.get("size"), Some(&Value::Unsigned(5)));
    assert_eq!(stat.get("kind"), Some(&s("file")));
    assert_eq!(stat.get("readonly"), Some(&Value::Bool(false)));
    assert!(matches!(stat.get("modified"), Some(Value::String(t)) if t.contains('T')));
    assert_eq!(
        read(&mut store, &result.join(&path!("size"))),
        Some(Value::Unsigned(5))
    );

    let dir_stat = write(&mut store, &path!("stat"), req(&[("path", os(dir.path()))])).unwrap();
    assert_eq!(
        read(&mut store, &dir_stat.join(&path!("kind"))),
        Some(s("dir"))
    );
    assert!(matches!(
        write(
            &mut store,
            &path!("stat"),
            req(&[("path", s("/nonexistent/x"))])
        ),
        Err(Error::Io(_))
    ));
}

#[test]
fn readdir_lists_sorted_entries() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("b.txt"), "").unwrap();
    std::fs::write(dir.path().join("a.txt"), "").unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let mut store = FsStore::new();
    let result = write(
        &mut store,
        &path!("readdir"),
        req(&[("path", os(dir.path()))]),
    )
    .unwrap();
    let entry = |name: &str, kind: &str| req(&[("name", s(name)), ("kind", s(kind))]);
    assert_eq!(
        read(&mut store, &result),
        Some(Value::Array(vec![
            entry("a.txt", "file"),
            entry("b.txt", "file"),
            entry("sub", "dir"),
        ]))
    );
}

#[test]
fn results_are_bounded_listed_and_discardable() {
    let (_dir, file) = fixture("f", "x");
    let mut store = FsStore::new();
    store.max_results = 2;
    for _ in 0..3 {
        write(&mut store, &path!("stat"), req(&[("path", os(&file))])).unwrap();
    }
    // The oldest answer was evicted.
    assert!(read(&mut store, &path!("results/0")).is_none());
    let Some(Value::Map(listing)) = read(&mut store, &path!("results")) else {
        panic!("expected listing")
    };
    assert!(matches!(listing.get("items"), Some(Value::Array(items)) if items.len() == 2));

    write(&mut store, &path!("results/1"), Value::Null).unwrap();
    assert!(read(&mut store, &path!("results/1")).is_none());
    assert!(matches!(
        write(&mut store, &path!("results/1"), Value::Null),
        Err(Error::NotFound { .. })
    ));
    assert!(matches!(
        write(&mut store, &path!("results/2"), s("x")),
        Err(Error::PermissionDenied { .. })
    ));
    assert!(matches!(
        write(&mut store, &path!("results/x"), Value::Null),
        Err(Error::InvalidArgument { .. })
    ));
}

#[test]
fn mkdir_rmdir_unlink_rename() {
    let dir = TempDir::new().unwrap();
    let mut store = FsStore::new();
    let deep = dir.path().join("a/b/c");
    assert!(write(&mut store, &path!("mkdir"), req(&[("path", os(&deep))])).is_err());
    write(
        &mut store,
        &path!("mkdir"),
        req(&[("path", os(&deep)), ("recursive", Value::Bool(true))]),
    )
    .unwrap();
    assert!(deep.is_dir());
    assert!(matches!(
        write(
            &mut store,
            &path!("mkdir"),
            req(&[("path", os(&deep)), ("recursive", s("yes"))]),
        ),
        Err(Error::InvalidArgument { .. })
    ));
    write(&mut store, &path!("rmdir"), req(&[("path", os(&deep))])).unwrap();
    assert!(!deep.exists());

    let old = dir.path().join("old");
    let new = dir.path().join("new");
    std::fs::write(&old, "x").unwrap();
    write(
        &mut store,
        &path!("rename"),
        req(&[("from", os(&old)), ("to", os(&new))]),
    )
    .unwrap();
    assert!(!old.exists() && new.exists());
    write(&mut store, &path!("unlink"), req(&[("path", os(&new))])).unwrap();
    assert!(!new.exists());

    for bad in [req(&[("from", os(&old))]), req(&[("to", os(&old))]), s("x")] {
        assert!(matches!(
            write(&mut store, &path!("rename"), bad),
            Err(Error::InvalidArgument { .. })
        ));
    }
}

#[test]
fn root_and_invalid_writes() {
    let mut store = FsStore::new();
    let Some(Value::Map(root)) = read(&mut store, &path!("")) else {
        panic!("expected root map")
    };
    for key in ["open", "handles", "results", "stat", "readdir", "mkdir"] {
        assert!(root.contains_key(key), "{key}");
    }
    assert!(read(&mut store, &path!("nonexistent")).is_none());
    assert!(matches!(
        write(&mut store, &path!(""), Value::Null),
        Err(Error::PermissionDenied { .. })
    ));
    assert!(matches!(
        write(&mut store, &path!("frobnicate"), req(&[])),
        Err(Error::InvalidArgument { .. })
    ));
    assert!(matches!(
        write(&mut store, &path!("open/extra"), req(&[])),
        Err(Error::InvalidArgument { .. })
    ));
}

// ---- confinement --------------------------------------------------------

#[test]
fn rooted_store_resolves_relative_paths_inside_root() {
    let dir = TempDir::new().unwrap();
    let mut store = FsStore::rooted(dir.path()).unwrap();
    assert_eq!(
        store.root(),
        Some(std::fs::canonicalize(dir.path()).unwrap().as_path())
    );
    let handle = open(&mut store, s("note.txt"), "write", Some("utf8"));
    write(&mut store, &handle, s("inside")).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("note.txt")).unwrap(),
        "inside"
    );
    write(
        &mut store,
        &path!("mkdir"),
        req(&[("path", s("x/y/z")), ("recursive", Value::Bool(true))]),
    )
    .unwrap();
    assert!(dir.path().join("x/y/z").is_dir());
    // `..` that stays inside the root is fine once it resolves.
    write(
        &mut store,
        &path!("stat"),
        req(&[("path", s("x/../note.txt"))]),
    )
    .unwrap();
    // An absolute path inside the root is accepted.
    let abs = std::fs::canonicalize(dir.path()).unwrap().join("note.txt");
    write(&mut store, &path!("stat"), req(&[("path", os(&abs))])).unwrap();
}

#[test]
fn rooted_store_rejects_escapes() {
    let outside = TempDir::new().unwrap();
    std::fs::write(outside.path().join("secret"), "s").unwrap();
    let dir = TempDir::new().unwrap();
    let mut store = FsStore::rooted(dir.path()).unwrap();

    let escapes = [
        os(&outside.path().join("secret")),
        s("../escape.txt"),
        s("missing/../../escape.txt"),
        s("/etc/passwd"),
    ];
    for path in escapes {
        let err = write(
            &mut store,
            &path!("open"),
            req(&[("path", path.clone()), ("mode", s("write"))]),
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                Error::PermissionDenied { .. } | Error::InvalidArgument { .. }
            ),
            "{path:?}: {err}"
        );
    }
    assert!(!dir.path().parent().unwrap().join("escape.txt").exists());
}

#[cfg(unix)]
#[test]
fn rooted_store_rejects_symlink_escape() {
    let outside = TempDir::new().unwrap();
    std::fs::write(outside.path().join("secret"), "s").unwrap();
    let dir = TempDir::new().unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("not-yet"), dir.path().join("dangling"))
        .unwrap();
    std::fs::write(dir.path().join("ok"), "ok").unwrap();
    std::os::unix::fs::symlink(dir.path().join("ok"), dir.path().join("inner")).unwrap();
    let mut store = FsStore::rooted(dir.path()).unwrap();

    for path in ["link/secret", "link/new-file", "dangling"] {
        let err = write(
            &mut store,
            &path!("open"),
            req(&[("path", s(path)), ("mode", s("write"))]),
        )
        .unwrap_err();
        assert!(
            matches!(err, Error::PermissionDenied { .. }),
            "{path}: {err}"
        );
    }
    assert!(!outside.path().join("not-yet").exists());
    assert!(!outside.path().join("new-file").exists());
    let err = write(&mut store, &path!("readdir"), req(&[("path", s("link"))])).unwrap_err();
    assert!(matches!(err, Error::PermissionDenied { .. }));

    // A symlink inside the root that stays inside is followed.
    let handle = open(&mut store, s("inner"), "read", Some("utf8"));
    assert_eq!(read(&mut store, &handle), Some(s("ok")));
    // Unlinking the escaping symlink removes the link, not its target.
    write(&mut store, &path!("unlink"), req(&[("path", s("link"))])).unwrap();
    assert!(outside.path().join("secret").exists());
}

#[test]
fn rooted_store_refuses_to_remove_or_move_the_root() {
    let dir = TempDir::new().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::write(root.join("f"), "x").unwrap();
    let mut store = FsStore::rooted(&root).unwrap();

    for spelling in [s("."), s(""), s("./"), os(&root), s("sub/..")] {
        for action in [path!("rmdir"), path!("unlink")] {
            let err = write(&mut store, &action, req(&[("path", spelling.clone())])).unwrap_err();
            assert!(
                matches!(
                    err,
                    Error::PermissionDenied { .. } | Error::InvalidArgument { .. }
                ),
                "{action} {spelling:?}: {err}"
            );
        }
        let err = write(
            &mut store,
            &path!("rename"),
            req(&[("from", spelling.clone()), ("to", s("moved"))]),
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                Error::PermissionDenied { .. } | Error::InvalidArgument { .. }
            ),
            "rename from {spelling:?}: {err}"
        );
        let err = write(
            &mut store,
            &path!("rename"),
            req(&[("from", s("f")), ("to", spelling.clone())]),
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                Error::PermissionDenied { .. } | Error::InvalidArgument { .. }
            ),
            "rename to {spelling:?}: {err}"
        );
    }
    // The plain spellings are refused specifically as the root.
    for action in [path!("rmdir"), path!("unlink")] {
        assert!(matches!(
            write(&mut store, &action, req(&[("path", s("."))])),
            Err(Error::PermissionDenied { .. })
        ));
    }
    std::fs::remove_file(root.join("f")).unwrap();
    // Even an empty root survives rmdir.
    assert!(write(&mut store, &path!("rmdir"), req(&[("path", s("."))])).is_err());
    assert!(root.is_dir());
}

#[cfg(unix)]
#[test]
fn rooted_store_confines_every_action() {
    let outside = TempDir::new().unwrap();
    std::fs::write(outside.path().join("secret"), "s").unwrap();
    std::fs::create_dir(outside.path().join("victim")).unwrap();
    let dir = TempDir::new().unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
    std::fs::write(dir.path().join("mine"), "m").unwrap();
    let mut store = FsStore::rooted(dir.path()).unwrap();

    let denied = |result: Result<Path, Error>, what: &str| {
        let err = result.unwrap_err();
        assert!(
            matches!(
                err,
                Error::PermissionDenied { .. } | Error::InvalidArgument { .. }
            ),
            "{what}: {err}"
        );
    };
    // rename: source outside, destination outside (direct and via symlink).
    denied(
        write(
            &mut store,
            &path!("rename"),
            req(&[
                ("from", os(&outside.path().join("secret"))),
                ("to", s("stolen")),
            ]),
        ),
        "rename source escape",
    );
    denied(
        write(
            &mut store,
            &path!("rename"),
            req(&[("from", s("link/secret")), ("to", s("stolen"))]),
        ),
        "rename source via symlink",
    );
    denied(
        write(
            &mut store,
            &path!("rename"),
            req(&[("from", s("mine")), ("to", s("../planted"))]),
        ),
        "rename destination escape",
    );
    denied(
        write(
            &mut store,
            &path!("rename"),
            req(&[("from", s("mine")), ("to", s("link/planted"))]),
        ),
        "rename destination via symlink",
    );
    // mkdir, rmdir, stat.
    denied(
        write(
            &mut store,
            &path!("mkdir"),
            req(&[("path", s("link/newdir"))]),
        ),
        "mkdir via symlink",
    );
    denied(
        write(
            &mut store,
            &path!("mkdir"),
            req(&[("path", s("../newdir"))]),
        ),
        "mkdir escape",
    );
    denied(
        write(
            &mut store,
            &path!("rmdir"),
            req(&[("path", s("link/victim"))]),
        ),
        "rmdir via symlink",
    );
    denied(
        write(
            &mut store,
            &path!("rmdir"),
            req(&[("path", os(&outside.path().join("victim")))]),
        ),
        "rmdir escape",
    );
    denied(
        write(
            &mut store,
            &path!("stat"),
            req(&[("path", s("link/secret"))]),
        ),
        "stat via symlink",
    );
    denied(
        write(&mut store, &path!("stat"), req(&[("path", s("../"))])),
        "stat escape",
    );
    // create_new under a symlinked parent.
    denied(
        write(
            &mut store,
            &path!("open"),
            req(&[("path", s("link/fresh")), ("mode", s("create_new"))]),
        ),
        "create_new under symlinked parent",
    );

    assert!(outside.path().join("secret").exists());
    assert!(outside.path().join("victim").is_dir());
    assert!(!outside.path().join("planted").exists());
    assert!(!outside.path().join("newdir").exists());
    assert!(!outside.path().join("fresh").exists());
    assert!(!dir.path().parent().unwrap().join("planted").exists());
    assert!(!dir.path().parent().unwrap().join("newdir").exists());
    assert!(dir.path().join("mine").exists());
}

#[test]
fn rooted_rename_from_subdirectory_into_root_succeeds() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub/f"), "x").unwrap();
    let mut store = FsStore::rooted(dir.path()).unwrap();
    write(
        &mut store,
        &path!("rename"),
        req(&[("from", s("sub/f")), ("to", s("f"))]),
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(dir.path().join("f")).unwrap(), "x");
    assert!(!dir.path().join("sub/f").exists());
}

#[test]
fn open_handles_are_bounded() {
    let (_dir, file) = fixture("f", "x");
    let mut store = FsStore::new().with_max_handles(2);
    let first = open(&mut store, os(&file), "read", None);
    open(&mut store, os(&file), "read", None);
    let err = write(
        &mut store,
        &path!("open"),
        req(&[("path", os(&file)), ("mode", s("read"))]),
    )
    .unwrap_err();
    assert!(matches!(err, Error::ResourceLimit { .. }), "{err}");
    // Closing one makes room.
    write(&mut store, &first.join(&path!("close")), Value::Null).unwrap();
    let third = open(&mut store, os(&file), "read", None);
    // Writing null to the handle itself closes it too.
    write(&mut store, &third, Value::Null).unwrap();
    assert!(matches!(
        write(&mut store, &third, Value::Null),
        Err(Error::NotFound { .. })
    ));
    open(&mut store, os(&file), "read", None);
    assert_eq!(FsStore::new().max_handles, DEFAULT_MAX_HANDLES);
}

#[test]
fn rooted_requires_existing_directory() {
    let (_dir, file) = fixture("f", "x");
    assert!(matches!(
        FsStore::rooted(&file),
        Err(Error::InvalidArgument { .. })
    ));
    assert!(FsStore::rooted("/nonexistent/root/12345").is_err());
}

// ---- meta lens ----------------------------------------------------------

#[test]
fn meta_lens_describes_actions() {
    let mut store = FsStore::new();
    let Some(Value::Map(root)) = read(&mut store, &path!("meta")) else {
        panic!("expected meta root")
    };
    for key in [
        "open", "handles", "stat", "readdir", "mkdir", "rmdir", "unlink", "rename",
    ] {
        assert!(root.contains_key(key), "{key}");
    }
    let open_modes = read(&mut store, &path!("meta/open/accepts/mode/values"));
    assert_eq!(
        open_modes,
        Some(Value::Array(OpenMode::NAMES.iter().map(|n| s(n)).collect()))
    );
    for action in ["stat", "readdir", "mkdir", "rmdir", "unlink", "rename"] {
        let Some(Value::Map(schema)) =
            read(&mut store, &Path::parse(&format!("meta/{action}")).unwrap())
        else {
            panic!("expected schema for {action}")
        };
        assert_eq!(schema.get("method"), Some(&s("write")));
        assert!(schema.contains_key("accepts"));
    }
    assert!(read(&mut store, &path!("meta/unknown")).is_none());
}

#[test]
fn meta_lens_describes_handles() {
    let (_dir, file) = fixture("f", "0123456789");
    let mut store = FsStore::new();
    open(&mut store, os(&file), "read", Some("utf8"));
    let Some(Value::Map(handle)) = read(&mut store, &path!("meta/handles/0")) else {
        panic!("expected handle meta")
    };
    let Some(Value::Map(state)) = handle.get("state") else {
        panic!("expected state")
    };
    assert_eq!(state.get("mode"), Some(&s("read")));
    assert_eq!(state.get("encoding"), Some(&s("utf8")));
    for key in ["position", "encoding", "close", "content", "at"] {
        assert!(handle.contains_key(key), "{key}");
    }

    write(
        &mut store,
        &path!("meta/handles/0/position"),
        Value::Integer(5),
    )
    .unwrap();
    assert_eq!(
        read(&mut store, &path!("meta/handles/0/position/value")),
        Some(Value::Integer(5))
    );
    for sub in ["meta", "at", "close"] {
        assert!(read(
            &mut store,
            &Path::parse(&format!("meta/handles/0/{sub}")).unwrap()
        )
        .is_some());
    }
    assert!(read(&mut store, &path!("meta/handles/0/unknown")).is_none());
    assert!(matches!(
        read(&mut store, &path!("meta/handles")),
        Some(Value::Map(_))
    ));
    assert!(matches!(
        store.read(&path!("meta/handles/abc")),
        Err(Error::InvalidArgument { .. })
    ));
    assert!(matches!(
        store.read(&path!("meta/handles/99")),
        Err(Error::NotFound { .. })
    ));
    for bad in [path!("meta/open"), path!("meta/handles/0/close")] {
        assert!(matches!(
            write(&mut store, &bad, Value::Integer(1)),
            Err(Error::PermissionDenied { .. })
        ));
    }
    assert!(matches!(
        write(
            &mut store,
            &path!("meta/handles/99/position"),
            Value::Integer(1)
        ),
        Err(Error::NotFound { .. })
    ));
    assert!(matches!(
        write(&mut store, &path!("meta/handles/0/position"), s("5")),
        Err(Error::InvalidArgument { .. })
    ));
}
