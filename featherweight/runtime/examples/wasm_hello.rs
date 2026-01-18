//! Example: Running a WASM Block through the Featherweight runtime.
//!
//! This example demonstrates loading and executing a WASM component
//! that implements the Block interface.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex as StdMutex};

use featherweight_runtime::{BlockId, Result, WasmBlock};
use structfs_core_store::{Error, NoCodec, Path, Reader, Record, Value, Writer};

/// Simple in-memory store for testing WASM Blocks.
struct InMemoryStore {
    data: Arc<StdMutex<BTreeMap<String, Value>>>,
}

impl InMemoryStore {
    fn shared(data: Arc<StdMutex<BTreeMap<String, Value>>>) -> Self {
        Self { data }
    }
}

impl Reader for InMemoryStore {
    fn read(&mut self, path: &Path) -> std::result::Result<Option<Record>, Error> {
        let path_str = path.to_string();
        let data = self.data.lock().unwrap();
        Ok(data.get(&path_str).cloned().map(Record::parsed))
    }
}

impl Writer for InMemoryStore {
    fn write(&mut self, path: &Path, record: Record) -> std::result::Result<Path, Error> {
        let path_str = path.to_string();
        let value = record.into_value(&NoCodec)?;
        let mut data = self.data.lock().unwrap();
        data.insert(path_str, value);
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
    let shared_data: Arc<StdMutex<BTreeMap<String, Value>>> =
        Arc::new(StdMutex::new(BTreeMap::new()));

    // Pre-populate some data for the Block to read
    {
        let mut data = shared_data.lock().unwrap();
        data.insert(
            "input/name".to_string(),
            Value::String("WASM World".to_string()),
        );
    }

    // Create a store for the Block
    let store = InMemoryStore::shared(shared_data.clone());

    // Run the WASM Block
    println!("\nRunning WASM Block...\n");
    let id = BlockId::new();
    block.run(id, store)?;

    // Check what the Block wrote
    println!("\nBlock execution complete. Checking results...\n");
    let data = shared_data.lock().unwrap();
    for (key, value) in data.iter() {
        println!("  {} = {:?}", key, value);
    }

    println!("\n=== Example complete ===");
    Ok(())
}
