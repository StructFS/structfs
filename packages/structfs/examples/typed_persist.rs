//! Typed reads and writes against a durable snapshot store.
//!
//! Run with `cargo run -p structfs --example typed_persist --features persist`.

use serde::{Deserialize, Serialize};
use structfs::persist::{BackedStore, Durability, JsonFileBacking};
use structfs::typed::{TypedReader, TypedWriter};
use structfs::{path, Error};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Server {
    host: String,
    port: u16,
}

fn main() -> Result<(), Error> {
    let file = std::env::temp_dir().join(format!("structfs-example-{}.json", std::process::id()));
    let backing = JsonFileBacking::new(&file).with_durability(Durability::Synced);
    let mut store = BackedStore::open(backing)?;

    let server = Server {
        host: "localhost".into(),
        port: 8080,
    };
    store.write_typed(&path!("config/server"), &server)?;

    // A second store over the same file sees the acknowledged write.
    let mut reopened = BackedStore::open(JsonFileBacking::new(&file))?;
    let loaded: Option<Server> = reopened.read_typed(&path!("config/server"))?;
    assert_eq!(loaded, Some(server));

    let _ = std::fs::remove_file(&file);
    Ok(())
}
