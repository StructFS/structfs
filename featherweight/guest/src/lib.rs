//! Featherweight Guest Library
//!
//! This crate provides the guest-side implementation for WASM Blocks.
//! It uses wit-bindgen to generate bindings from the WIT file.

// Generate bindings from the WIT file
wit_bindgen::generate!({
    world: "block-world",
    path: "wit/world.wit",
});

use exports::featherweight::block::block::Guest;
use featherweight::block::store::{read, write, ReadResult, Value, WriteResult};

/// A simple hello world Block implementation.
struct HelloBlock;

impl Guest for HelloBlock {
    fn run() -> Result<(), String> {
        // Read name from input path
        let name = match read("input/name") {
            ReadResult::Found(Value::ValText(s)) => s,
            ReadResult::Found(_) => return Err("input/name must be a string".to_string()),
            ReadResult::NotFound => "World".to_string(),
            ReadResult::ReadError(e) => return Err(format!("Failed to read input/name: {}", e)),
        };

        // Create greeting
        let greeting = format!("Hello, {}!", name);

        // Write greeting to output path
        match write("output/greeting", &Value::ValText(greeting.clone())) {
            WriteResult::Written(_) => {}
            WriteResult::WriteError(e) => {
                return Err(format!("Failed to write output/greeting: {}", e))
            }
        }

        // Also write a status message
        match write("output/status", &Value::ValText("completed".to_string())) {
            WriteResult::Written(_) => {}
            WriteResult::WriteError(e) => {
                return Err(format!("Failed to write output/status: {}", e))
            }
        }

        Ok(())
    }
}

// Export the Block implementation
export!(HelloBlock);
