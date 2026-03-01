//! Featherweight Guest Library
//!
//! This crate provides the guest-side implementation for WASM Blocks.
//! It uses wit-bindgen to generate bindings from the WIT file.
//!
//! The WIT interface speaks raw bytes: paths are `list<list<u8>>` (byte
//! components) and data is `list<u8>`. Guests use a serialization format
//! (e.g. JSON) to encode/decode structured data.

// Generate bindings from the WIT file
wit_bindgen::generate!({
    world: "block-world",
    path: "wit/world.wit",
});

use exports::featherweight::block::block::Guest;
use featherweight::block::ll_store::{read, write};

/// A simple hello world Block implementation.
struct HelloBlock;

impl Guest for HelloBlock {
    fn manifest() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "name": "hello-block",
            "version": "0.1.0",
            "serialization": "application/json",
            "paths": {
                "input/name": { "read": "Name to greet" },
                "output/greeting": { "write": "Generated greeting" },
                "output/status": { "write": "Completion status" }
            }
        }))
        .unwrap()
    }

    fn run() -> Result<(), String> {
        // Read name from input path (JSON-encoded)
        let name = match read(&[b"input".to_vec(), b"name".to_vec()])
            .map_err(|e| format!("Failed to read input/name: {}", e))?
        {
            Some(bytes) => {
                serde_json::from_slice::<String>(&bytes).unwrap_or_else(|_| "World".to_string())
            }
            None => "World".to_string(),
        };

        // Create greeting
        let greeting = format!("Hello, {}!", name);

        // Write greeting to output path (JSON-encoded)
        let data =
            serde_json::to_vec(&greeting).map_err(|e| format!("Failed to serialize: {}", e))?;
        write(&[b"output".to_vec(), b"greeting".to_vec()], &data)
            .map_err(|e| format!("Failed to write output/greeting: {}", e))?;

        // Also write a status message (JSON-encoded)
        let status_data =
            serde_json::to_vec(&"completed").map_err(|e| format!("Failed to serialize: {}", e))?;
        write(&[b"output".to_vec(), b"status".to_vec()], &status_data)
            .map_err(|e| format!("Failed to write output/status: {}", e))?;

        Ok(())
    }
}

// Export the Block implementation
export!(HelloBlock);
