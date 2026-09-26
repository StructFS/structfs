//! `use structfs::*` must not shadow well-known crates. Before 0.5 the facade
//! had modules named `serde`, `json`, `http` and `sys`; a glob import then
//! made `serde::Serialize` ambiguous (E0659). With every feature enabled
//! (`--all-features`), this file compiling is the check.

use serde::{Deserialize, Serialize};
use structfs::*;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Greeting {
    text: String,
}

#[test]
fn glob_import_coexists_with_serde() {
    let mut store = MemoryStore::new();
    store
        .write(&path!("greeting"), Record::parsed(Value::from("hello")))
        .unwrap();
    assert!(store.read(&path!("greeting")).unwrap().is_some());

    let greeting = Greeting {
        text: "hello".into(),
    };
    // Fully qualified paths through the `serde` crate still resolve.
    fn _is_serde<T: serde::Serialize + serde::de::DeserializeOwned>(_: &T) {}
    _is_serde(&greeting);

    #[cfg(feature = "typed")]
    {
        let value = typed::to_value(&greeting).unwrap();
        let back: Greeting = typed::from_value(value).unwrap();
        assert_eq!(back, greeting);
    }
}
