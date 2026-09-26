//! Guest metering: the per-run fuel cap.
//!
//! Store operations are governed by deadlines and cancellation; guest
//! *code* between host calls is governed by two engine mechanisms:
//!
//! - **Epoch interruption** is an engine concern, always on in both wasm
//!   bindings. One ticker per engine advances the epoch (every 10 ms by
//!   default; see [`crate::CoreWasmEngine::with_epoch_interval`]); at each
//!   tick a running guest checks its cancellation token and deadline and is
//!   trapped once either has fired, otherwise it continues (or yields, when
//!   run asynchronously). Parked host calls are unaffected — a block
//!   waiting on its mailbox is *supposed* to wait. There is no per-run
//!   epoch knob: a run cannot change a shared engine's ticker, and turning
//!   interruption off would make a spinning guest unstoppable.
//! - **Fuel** is per run, and this type selects it: `None` (the default)
//!   leaves fuel unbounded but still counted; `Some(n)` traps the guest
//!   after `n` units of Wasmtime fuel.
//!
//! Both the core-wasm binding and the component adapter honour the same
//! `Metering` value.

/// Per-run metering for wasm guests (both bindings).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Metering {
    /// Total Wasmtime fuel per run; `None` means unbounded (still counted).
    pub fuel: Option<u64>,
}

impl Metering {
    /// Cap each run at `fuel` units of Wasmtime fuel.
    pub fn with_fuel(fuel: u64) -> Self {
        Self { fuel: Some(fuel) }
    }
}
