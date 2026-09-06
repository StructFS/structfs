//! Guest metering: fuel caps and epoch interruption.
//!
//! Store operations are governed by deadlines and cancellation, but
//! guest *code* between host calls was previously ungoverned — a
//! spinning guest could not be stopped. Metering closes that:
//!
//! - **Epoch interruption** (default on): a ticker advances the engine
//!   epoch; at every deadline the guest yields to a callback that traps
//!   it if its block's cancel token has fired (immediate shutdown) and
//!   otherwise lets it continue. Parked host calls are unaffected — a
//!   block waiting on its mailbox is *supposed* to wait.
//! - **Fuel** (default off): a hard cap on total guest instructions per
//!   run, for hosts that want budgeted execution.
//!
//! Adapted from the ox runtime's engine configuration, which proved the
//! parked-vs-spinning distinction matters: epoch/fuel govern guest
//! execution only, never store waits.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use structfs_handles::CancelToken;
use wasmtime::UpdateDeadline;

/// Metering configuration for wasm guests (both bindings).
#[derive(Clone, Debug)]
pub struct Metering {
    /// Total fuel per run; `None` disables fuel accounting.
    pub fuel: Option<u64>,
    /// Epoch tick interval; `None` disables interruption (a spinning
    /// guest then cannot be stopped).
    pub epoch_interval: Option<Duration>,
}

impl Default for Metering {
    fn default() -> Self {
        Self {
            fuel: None,
            epoch_interval: Some(Duration::from_millis(10)),
        }
    }
}

impl Metering {
    /// No fuel, no interruption — for tests and fully trusted guests.
    /// A spinning guest cannot be stopped under this configuration.
    pub fn disabled() -> Self {
        Self {
            fuel: None,
            epoch_interval: None,
        }
    }

    /// Apply engine-level settings. Public for binding adapters, which
    /// build their own engines but must meter guests identically.
    pub fn configure_engine(&self, config: &mut wasmtime::Config) {
        if self.fuel.is_some() {
            config.consume_fuel(true);
        }
        if self.epoch_interval.is_some() {
            config.epoch_interruption(true);
        }
    }

    /// Arm a store: fuel budget, and an epoch deadline whose callback
    /// traps the guest once `cancel` fires.
    pub fn arm_store<T>(
        &self,
        store: &mut wasmtime::Store<T>,
        cancel: CancelToken,
    ) -> crate::error::Result<()> {
        if let Some(fuel) = self.fuel {
            store
                .set_fuel(fuel)
                .map_err(|e| crate::error::RuntimeError::wasm("fuel", e))?;
        }
        if self.epoch_interval.is_some() {
            store.set_epoch_deadline(1);
            store.epoch_deadline_callback(move |_| {
                if cancel.is_cancelled() {
                    Err(wasmtime::Error::msg(
                        "guest interrupted: immediate shutdown",
                    ))
                } else {
                    Ok(UpdateDeadline::Continue(1))
                }
            });
        }
        Ok(())
    }

    /// Start the epoch ticker for an engine, if interruption is enabled.
    /// The ticker stops when the returned guard drops.
    pub fn start_ticker(&self, engine: &wasmtime::Engine) -> Option<EpochTicker> {
        let interval = self.epoch_interval?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let engine = engine.clone();
        let handle = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                std::thread::sleep(interval);
                engine.increment_epoch();
            }
        });
        Some(EpochTicker {
            stop,
            handle: Some(handle),
        })
    }
}

/// Stops the epoch ticker thread on drop.
pub struct EpochTicker {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for EpochTicker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
