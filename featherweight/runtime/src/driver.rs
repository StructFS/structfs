//! Public execution context for artifact adapters. Preparation belongs to the
//! embedding host; each run receives a fresh namespace and instance accounting.
use crate::{BlockId, CallBudget, ExecutionScope, Metering, Namespace};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use structfs_core_store::Format;
use structfs_handles::CancelToken;

/// A measurement with a stable, explicit unit. Driver counters must not be
/// interpreted as host-enforced limits (e.g. x86 instructions are not Wasm fuel).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DriverCounter {
    pub unit: String,
    pub value: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionUsage {
    /// None means this driver does not report the measurement.
    pub wasm_fuel_consumed: Option<u64>,
    pub wasm_fuel_limit: Option<u64>,
    pub linear_memory_limit_bytes: Option<usize>,
    pub linear_memory_bytes: Option<usize>,
    pub peak_linear_memory_bytes: Option<usize>,
    pub counters: BTreeMap<String, DriverCounter>,
    /// Set by the runtime after execution terminates, including failures.
    pub finished: bool,
}
/// Instance-local accounting, retained after teardown. Samples are observations;
/// limits are enforced separately by the driver/engine and admission budgets.
#[derive(Clone, Default)]
pub struct ExecutionMeter(Arc<Mutex<ExecutionUsage>>);
impl ExecutionMeter {
    pub fn snapshot(&self) -> ExecutionUsage {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn configure_wasm(&self, fuel: Option<u64>, memory: Option<usize>) {
        let mut usage = self.0.lock().unwrap_or_else(|e| e.into_inner());
        usage.wasm_fuel_limit = fuel;
        usage.linear_memory_limit_bytes = memory;
    }
    pub fn sample_fuel(&self, fuel: u64) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .wasm_fuel_consumed = Some(fuel);
    }
    pub fn sample_wasm(&self, fuel: u64, memory: usize) {
        let mut usage = self.0.lock().unwrap_or_else(|e| e.into_inner());
        usage.wasm_fuel_consumed = Some(fuel);
        usage.linear_memory_bytes = Some(memory);
        usage.peak_linear_memory_bytes =
            Some(usage.peak_linear_memory_bytes.unwrap_or(0).max(memory));
    }
    /// Bounded diagnostic labels; counters cannot change units once introduced.
    pub fn counter(
        &self,
        name: &str,
        unit: &str,
        value: u64,
    ) -> Result<(), structfs_core_store::Error> {
        let mut usage = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if name.len() > 128 || unit.len() > 64 || name.is_empty() || unit.is_empty() {
            return Err(structfs_core_store::Error::resource_limit(
                "invalid counter label",
            ));
        }
        if let Some(old) = usage.counters.get(name) {
            if old.unit != unit {
                return Err(structfs_core_store::Error::conflict("counter unit changed"));
            }
        } else if usage.counters.len() >= 64 {
            return Err(structfs_core_store::Error::resource_limit(
                "too many driver counters",
            ));
        }
        usage.counters.insert(
            name.into(),
            DriverCounter {
                unit: unit.into(),
                value,
            },
        );
        Ok(())
    }
    pub(crate) fn finish(&self) {
        let mut usage = self.0.lock().unwrap_or_else(|e| e.into_inner());
        usage.finished = true;
        if usage.linear_memory_bytes.is_some() {
            usage.linear_memory_bytes = Some(0);
        }
    }
}

/// Passed through the same entry point to built-in and external drivers.
/// Drivers own their execution resources until this run returns. Async drivers
/// must release parked operations on cancellation and join child tasks before
/// returning. A blocking driver must cooperate; it cannot be forcibly stopped.
pub struct DriverContext {
    pub id: BlockId,
    pub namespace: Namespace,
    pub format: Format,
    pub metering: Metering,
    pub cancel: CancelToken,
    pub execution: Option<ExecutionScope>,
    pub calls: Arc<CallBudget>,
    pub usage: ExecutionMeter,
}

/// Optional adapter-owned control surface. Only the adapter can establish its
/// safe points. Advertising support does not make arbitrary Wasm suspendable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DriverCapabilities {
    pub resumable: bool,
    pub checkpoints: bool,
}
/// Host-only controls, never implicitly granted to guest namespaces. Operations
/// are adapter-defined paths with documented schemas; unsupported operations
/// must fail explicitly. Checkpoints must include/validate provider state.
pub type DriverControl = crate::HostStore;
