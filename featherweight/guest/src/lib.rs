//! Featherweight Guest Library
//!
//! The guest side of a wasm Block. The WIT boundary speaks raw bytes:
//! paths are `list<list<u8>>` (byte components) and data is `list<u8>` in
//! the block's declared serialization format (JSON here). The host's
//! `CoreToLL` bridge encodes/decodes `Value`s with the codec selected
//! from this block's `manifest()`.
//!
//! This crate implements a real Isotope block
//! (`isotope/spec/07-server-protocol.md`): a kv store that loops reading
//! `iso/server/requests`, serves each request from an in-memory map, and
//! writes responses to the request's `respond_to` path, until shutdown
//! unblocks the request read with `null`.

// Generate bindings from the canonical Block ABI (single-sourced;
// the runtime's bindings come from the same file).
wit_bindgen::generate!({
    world: "block-world",
    path: "../wit/world.wit",
});

use std::collections::HashMap;

use exports::featherweight::block::block::Guest;
use featherweight::block::ll_store::{read, write};

/// Split a path string into the byte components the WIT boundary wants.
fn path_components(path: &str) -> Vec<Vec<u8>> {
    path.split('/')
        .filter(|c| !c.is_empty())
        .map(|c| c.as_bytes().to_vec())
        .collect()
}

fn read_json(path: &str) -> Result<Option<serde_json::Value>, String> {
    match read(&path_components(path)).map_err(|e| format!("read {path}: {e}"))? {
        Some(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("decode {path}: {e}")),
        None => Ok(None),
    }
}

fn write_json(path: &str, value: &serde_json::Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| format!("encode {path}: {e}"))?;
    write(&path_components(path), &bytes).map_err(|e| format!("write {path}: {e}"))?;
    Ok(())
}

/// A kv-store Block served over the server protocol.
struct KvBlock;

impl Guest for KvBlock {
    fn manifest() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "name": "wasm-kv",
            "version": "0.2.0",
            "serialization": "application/json",
            "paths": {
                "/{key}": {
                    "read": "Get a stored value",
                    "write": "Store a value"
                }
            }
        }))
        .unwrap()
    }

    fn run() -> Result<(), String> {
        write_json(
            "iso/self/interface",
            &serde_json::json!({"name": "wasm-kv", "paths": {"/{key}": {"read": true, "write": true}}}),
        )?;

        let mut store: HashMap<String, serde_json::Value> = HashMap::new();

        // Blocking read: parks until a request arrives; `null` means
        // shutdown was requested.
        while let Some(request) = read_json("iso/server/requests")? {
            if request.is_null() {
                break;
            }

            let op = request.get("op").and_then(|v| v.as_str()).unwrap_or("");
            let path = request.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let Some(respond_to) = request.get("respond_to").and_then(|v| v.as_str()) else {
                continue;
            };

            let response = match op {
                "read" => {
                    let value = store.get(path).cloned().unwrap_or(serde_json::Value::Null);
                    serde_json::json!({"result": "ok", "value": value})
                }
                "write" => {
                    let data = request
                        .get("data")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    if data.is_null() {
                        store.remove(path);
                    } else {
                        store.insert(path.to_string(), data);
                    }
                    serde_json::json!({"result": "ok", "path": path})
                }
                other => serde_json::json!({
                    "result": "error",
                    "error": {
                        "type": "store_error",
                        "message": format!("unknown op: {other}"),
                        "retryable": false
                    }
                }),
            };
            write_json(respond_to, &response)?;
        }

        write_json("iso/shutdown/complete", &serde_json::json!({}))?;
        Ok(())
    }
}

// Export the Block implementation
export!(KvBlock);
