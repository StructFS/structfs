//! Example: Running a WASM Block through the Featherweight runtime.
//!
//! This example demonstrates loading and executing a WASM component
//! that implements the Block interface.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex as StdMutex};

use featherweight_runtime::{BlockId, Result, WasmBlock};
use structfs_core_store::{Bytes, Error, Format, NoCodec, Path, Reader, Record, Writer};

/// Simple in-memory store for testing WASM Blocks.
///
/// Stores raw bytes keyed by path strings. Used with `Format::JSON` so the
/// WASM guest can parse/produce JSON-encoded data.
struct InMemoryStore {
    data: Arc<StdMutex<BTreeMap<String, Vec<u8>>>>,
    format: Format,
}

impl InMemoryStore {
    fn shared(data: Arc<StdMutex<BTreeMap<String, Vec<u8>>>>, format: Format) -> Self {
        Self { data, format }
    }
}

impl Reader for InMemoryStore {
    fn read(&mut self, path: &Path) -> std::result::Result<Option<Record>, Error> {
        let path_str = path.to_string();
        let data = self.data.lock().unwrap();
        Ok(data
            .get(&path_str)
            .map(|b| Record::raw(Bytes::from(b.clone()), self.format.clone())))
    }
}

impl Writer for InMemoryStore {
    fn write(&mut self, path: &Path, record: Record) -> std::result::Result<Path, Error> {
        let bytes = record.into_bytes(&NoCodec, &self.format)?;
        let mut data = self.data.lock().unwrap();
        data.insert(path.to_string(), bytes.to_vec());
        Ok(path.clone())
    }
}

fn main() -> Result<()> {
    println!("=== Featherweight WASM Block Example ===\n");

    // Check for WASM component file argument
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <path-to-wasm-component>", args[0]);
        eprintln!("\nTo create a WASM component, compile a guest Block with:");
        eprintln!("  cargo component build --release -p featherweight-guest");
        std::process::exit(1);
    }

    let wasm_path = &args[1];
    println!("Loading WASM component from: {}", wasm_path);

    // Load the WASM Block
    let block = WasmBlock::from_file(wasm_path)?;

    // Create shared storage
    let shared_data: Arc<StdMutex<BTreeMap<String, Vec<u8>>>> =
        Arc::new(StdMutex::new(BTreeMap::new()));

    // Pre-populate some data for the Block to read (as JSON bytes)
    {
        let mut data = shared_data.lock().unwrap();
        data.insert(
            "input/name".to_string(),
            serde_json::to_vec(&"WASM World").unwrap(),
        );
    }

    // Create a store for the Block
    let format = Format::JSON;
    let store = InMemoryStore::shared(shared_data.clone(), format.clone());

    // Run the WASM Block with NoCodec (data is already in JSON format)
    println!("\nRunning WASM Block...\n");
    let id = BlockId::new();
    block.run(id, store, NoCodec, format)?;

    // Check what the Block wrote
    println!("\nBlock execution complete. Checking results...\n");
    let data = shared_data.lock().unwrap();
    for (key, value) in data.iter() {
        let display = String::from_utf8_lossy(value);
        println!("  {} = {}", key, display);
    }

    println!("\n=== Example complete ===");
    Ok(())
}
