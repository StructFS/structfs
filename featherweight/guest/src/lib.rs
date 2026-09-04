//! Featherweight Guest Library — the core-wasm binding
//! (`isotope/spec/11-core-wasm-binding.md`).
//!
//! This crate is both the Rust guest SDK for the core binding (the
//! `sdk` module — two imports from the `structfs` module, a
//! `block_alloc` export, and safe wrappers) and the reference guest: a
//! kv store served over the Isotope server protocol
//! (`isotope/spec/07-server-protocol.md`).
//!
//! Build with plain `cargo build --target wasm32-unknown-unknown` — no
//! componentization, no bindgen. The whole ABI surface is the `sdk`
//! module; everything below `kv` is ordinary library code.

use std::collections::HashMap;

/// The core-binding SDK: the entire ABI surface for a Rust guest.
pub mod sdk {
    /// The ret record (spec 11): `{ptr, len}`, little-endian u32s.
    #[repr(C)]
    pub struct Ret {
        pub ptr: u32,
        pub len: u32,
    }

    #[link(wasm_import_module = "structfs")]
    extern "C" {
        fn read(path_ptr: *const u8, path_len: i32, ret_ptr: *mut Ret) -> i32;
        fn write(
            path_ptr: *const u8,
            path_len: i32,
            data_ptr: *const u8,
            data_len: i32,
            ret_ptr: *mut Ret,
        ) -> i32;
    }

    /// The guest's one obligation: hand the host writable memory.
    #[no_mangle]
    pub extern "C" fn block_alloc(len: i32) -> *mut u8 {
        if len <= 0 {
            return std::ptr::null_mut();
        }
        let layout = std::alloc::Layout::from_size_align(len as usize, 1).unwrap();
        unsafe { std::alloc::alloc(layout) }
    }

    /// Take ownership of a host-filled ret buffer.
    fn take(ret: Ret) -> Vec<u8> {
        if ret.len == 0 {
            Vec::new()
        } else {
            // Reclaim the buffer block_alloc handed out (size == len,
            // align 1 — compatible with Vec<u8>'s layout).
            unsafe { Vec::from_raw_parts(ret.ptr as *mut u8, ret.len as usize, ret.len as usize) }
        }
    }

    fn message(ret: Ret) -> String {
        String::from_utf8_lossy(&take(ret)).into_owned()
    }

    /// StructFS read: `Ok(Some(bytes))`, `Ok(None)` when absent, or the
    /// host's diagnostic message on error.
    pub fn structfs_read(path: &str) -> Result<Option<Vec<u8>>, String> {
        let mut ret = Ret { ptr: 0, len: 0 };
        let status = unsafe { read(path.as_ptr(), path.len() as i32, &mut ret) };
        match status {
            0 => Ok(Some(take(ret))),
            1 => Ok(None),
            _ => Err(message(ret)),
        }
    }

    /// StructFS write: the result path, or the host's diagnostic message.
    pub fn structfs_write(path: &str, data: &[u8]) -> Result<String, String> {
        let mut ret = Ret { ptr: 0, len: 0 };
        let status = unsafe {
            write(
                path.as_ptr(),
                path.len() as i32,
                data.as_ptr(),
                data.len() as i32,
                &mut ret,
            )
        };
        match status {
            0 => Ok(message(ret)),
            _ => Err(message(ret)),
        }
    }
}

const MANIFEST: &str = r#"{"name":"wasm-kv","version":"0.3.0","serialization":"application/json","paths":{"/{key}":{"read":"Get a stored value","write":"Store a value"}}}"#;

/// Guest export: the manifest, pre-wiring (spec 01).
///
/// # Safety
///
/// `ret_ptr` must point to a valid, writable ret record; the host
/// guarantees this per the binding contract (spec 11).
#[no_mangle]
pub unsafe extern "C" fn manifest(ret_ptr: *mut sdk::Ret) -> i32 {
    (*ret_ptr).ptr = MANIFEST.as_ptr() as u32;
    (*ret_ptr).len = MANIFEST.len() as u32;
    0
}

fn read_json(path: &str) -> Result<Option<serde_json::Value>, String> {
    match sdk::structfs_read(path)? {
        Some(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("decode {path}: {e}")),
        None => Ok(None),
    }
}

fn write_json(path: &str, value: &serde_json::Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| format!("encode {path}: {e}"))?;
    sdk::structfs_write(path, &bytes)?;
    Ok(())
}

/// The block's main: a kv store served over the server protocol.
fn kv_main() -> Result<(), String> {
    write_json(
        "iso/self/interface",
        &serde_json::json!({"name": "wasm-kv", "paths": {"/{key}": {"read": true, "write": true}}}),
    )?;

    let mut store: HashMap<String, serde_json::Value> = HashMap::new();

    // Blocking mailbox read: parks until an event; `null` unblocks on
    // shutdown.
    while let Some(request) = read_json("iso/server/requests")? {
        if request.is_null() {
            break;
        }

        let op = request.get("op").and_then(|v| v.as_str()).unwrap_or("");
        let path = request.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let Some(respond_to) = request.get("respond_to").and_then(|v| v.as_str()) else {
            continue; // signals and timers carry no respond_to
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

/// Guest export: the block's main. Nonzero is the exit code.
#[no_mangle]
pub extern "C" fn run() -> i32 {
    match kv_main() {
        Ok(()) => 0,
        Err(message) => {
            // Best-effort diagnostic through the store before exiting.
            let _ = write_json("iso/log/error", &serde_json::json!({ "msg": message }));
            1
        }
    }
}
