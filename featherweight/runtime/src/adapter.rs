//! Engine hooks for binding adapters (hidden from the documented API).
//!
//! The runtime core knows only the Block ABI and its own core-wasm
//! binding; it has no WIT or component-model knowledge. Adapters that
//! run other artifact kinds on Wasmtime (such as `featherweight-component`)
//! must still configure, tick, and arm their engines exactly as the core
//! binding does, or metering and cancellation would differ between
//! bindings. These hooks are that shared policy.
//!
//! They expose Wasmtime types, so they are `#[doc(hidden)]` and outside
//! the crate's semver contract: an adapter must depend on the same
//! Wasmtime release as this crate. [`wasmtime`] is re-exported here so an
//! adapter can name the exact types the runtime was built against.

use std::time::Duration;

use crate::error::{Result, RuntimeError};

pub use wasmtime;

/// The default epoch tick: how often a running guest checks cancellation.
pub const DEFAULT_EPOCH_INTERVAL: Duration = Duration::from_millis(10);

/// Fuel granted to a guest's `manifest` export: bounded, so a misbehaving
/// manifest cannot hang the loader.
pub const MANIFEST_FUEL: u64 = 10_000_000;

/// Engine settings every binding shares: deterministic floating point
/// (spec 12, runtime obligation 2 — the same guest bytes on the same
/// answers compute the same result on every host), fuel accounting, and
/// epoch interruption.
pub fn configure_engine(config: &mut wasmtime::Config) {
    config.cranelift_nan_canonicalization(true);
    config.relaxed_simd_deterministic(true);
    config.consume_fuel(true);
    config.epoch_interruption(true);
}

/// Advances one engine's epoch on a Tokio timer task; stops on drop.
pub struct EpochTicker(tokio::task::JoinHandle<()>);

impl Drop for EpochTicker {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Start `engine`'s ticker on the current Tokio runtime.
pub fn start_ticker(engine: &wasmtime::Engine, interval: Duration) -> Result<EpochTicker> {
    if interval.is_zero() {
        return Err(RuntimeError::EngineConfig(
            "epoch interval must be positive".into(),
        ));
    }
    let handle = tokio::runtime::Handle::try_current().map_err(|_| {
        RuntimeError::EngineConfig("create the engine inside a live Tokio runtime".into())
    })?;
    let engine = engine.clone();
    Ok(EpochTicker(handle.spawn(async move {
        let mut ticks = tokio::time::interval(interval);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticks.tick().await;
            engine.increment_epoch();
        }
    })))
}

/// Arm a store for one run: its fuel budget (`None` = unbounded) and an
/// epoch callback that traps the guest once `check` fails, otherwise
/// continues — or yields to the executor, for asynchronous runs.
pub fn arm_store<T: 'static>(
    store: &mut wasmtime::Store<T>,
    fuel: Option<u64>,
    asynchronous: bool,
    check: impl Fn() -> std::result::Result<(), String> + Send + Sync + 'static,
) -> Result<()> {
    store
        .set_fuel(fuel.unwrap_or(u64::MAX))
        .map_err(|e| RuntimeError::wasm("fuel", e))?;
    if asynchronous {
        store
            .fuel_async_yield_interval(Some(100_000))
            .map_err(|e| RuntimeError::wasm("fuel yield", e))?;
    }
    store.set_epoch_deadline(1);
    store.epoch_deadline_callback(move |_| {
        check().map_err(wasmtime::Error::msg)?;
        Ok(if asynchronous {
            wasmtime::UpdateDeadline::Yield(1)
        } else {
            wasmtime::UpdateDeadline::Continue(1)
        })
    });
    Ok(())
}
