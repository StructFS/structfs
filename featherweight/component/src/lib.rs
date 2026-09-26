//! The WIT component-model binding adapter.
//!
//! The featherweight core knows only the Block ABI (spec 10) and its
//! core-wasm binding (spec 11); it has no idea what WIT is. This crate
//! is one more way to get things running as Isotope blocks — wasm
//! components built with wit-bindgen / wasip2 tooling — packaged as an
//! [`ArtifactLoader`] the embedder registers:
//!
//! ```ignore
//! let mut config = featherweight_runtime::RuntimeConfig::new(handle);
//! featherweight_component::register(&mut config);
//! let runtime = featherweight_runtime::Runtime::new(config);
//! ```
//!
//! The WASM boundary is an LL-store boundary: the WIT interface speaks raw
//! bytes (`list<u8>` for data, `list<list<u8>>` for paths). The adapter wraps
//! the Block's root store in a `CoreToLL` bridge with the Block's declared
//! codec and format, so the host implementation is a thin forward to
//! `ll_read`/`ll_write`. The WIT never changes when serialization formats do.
//!
//! Like the core binding, a component is compiled and its manifest inspected
//! once ([`ComponentEngine::prepare`]); every run gets a fresh store on the
//! shared engine, whose epoch ticker interrupts guests on cancellation and
//! whose fuel accounting honours [`featherweight_runtime::Metering`].

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use bytes::Bytes;
use structfs_core_store::{Codec, CoreToLL, Format, NoCodec, Reader, Writer};
use structfs_handles::CancelToken;
use structfs_ll_store::{LLReader, LLWriter};
use structfs_serde_store::MultiCodec;
use wasmtime::component::{bindgen, Component, Linker};
use wasmtime::{Config, Engine, Store};

use featherweight_runtime::adapter;
use featherweight_runtime::core_wasm::is_component;
use featherweight_runtime::{
    ArtifactLoader, DriverContext, NoOpStore, Result, RuntimeConfig, RuntimeError, WasmBlockDriver,
};

// Generate bindings from the component projection of the Block ABI
// (wit/world.wit; spec 10 is the source of truth).
bindgen!({
    path: "wit/world.wit",
    world: "block-world",
});

/// Fuel for a component's `manifest` export: component instantiation runs
/// guest initialisers, so the bound is looser than the core binding's.
const MANIFEST_FUEL: u64 = 10_000_000_000;

/// State held by the Wasmtime store for each Block run: the Block's root
/// store, wrapped in a CoreToLL bridge. The Block world declares no
/// resources, so no resource table is needed.
struct WasmBlockState<S, C> {
    root: Arc<Mutex<CoreToLL<S, C>>>,
}

impl<S, C> WasmBlockState<S, C> {
    fn new(root: S, codec: C, format: Format) -> Self {
        Self {
            root: Arc::new(Mutex::new(CoreToLL::new(root, codec, format))),
        }
    }

    /// A store that panicked mid-operation may be inconsistent, but the next
    /// call observes the damage rather than failing forever.
    fn root(&self) -> MutexGuard<'_, CoreToLL<S, C>> {
        self.root.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Implementation of the ll-store interface for WASM Blocks.
impl<S: Reader + Writer + Send + 'static, C: Codec + Send + Sync + 'static>
    featherweight::block::ll_store::Host for WasmBlockState<S, C>
{
    fn read(&mut self, path: Vec<Vec<u8>>) -> std::result::Result<Option<Vec<u8>>, String> {
        let path_refs: Vec<&[u8]> = path.iter().map(|c| c.as_slice()).collect();
        match self.root().ll_read(&path_refs) {
            Ok(Some(bytes)) => Ok(Some(bytes.to_vec())),
            Ok(None) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn write(
        &mut self,
        path: Vec<Vec<u8>>,
        data: Vec<u8>,
    ) -> std::result::Result<Vec<Vec<u8>>, String> {
        let path_refs: Vec<&[u8]> = path.iter().map(|c| c.as_slice()).collect();
        match self.root().ll_write(&path_refs, Bytes::from(data)) {
            Ok(result_path) => Ok(result_path.into_iter().map(|b| b.to_vec()).collect()),
            Err(e) => Err(e.to_string()),
        }
    }
}

fn wasm(operation: &'static str) -> impl Fn(wasmtime::Error) -> RuntimeError {
    move |e| RuntimeError::wasm(operation, format!("{e:#}"))
}

/// One component-model engine and its epoch ticker, shared by every
/// component it prepares. Create it inside a Tokio runtime.
pub struct ComponentEngine {
    engine: Engine,
    _ticker: adapter::EpochTicker,
}

impl ComponentEngine {
    /// An engine with the default 10 ms epoch tick.
    pub fn new() -> Result<Arc<Self>> {
        Self::with_epoch_interval(adapter::DEFAULT_EPOCH_INTERVAL)
    }

    /// An engine whose ticker interrupts cancelled guests every `interval`.
    pub fn with_epoch_interval(interval: std::time::Duration) -> Result<Arc<Self>> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        adapter::configure_engine(&mut config);
        let engine = Engine::new(&config).map_err(|e| RuntimeError::EngineConfig(e.to_string()))?;
        let ticker = adapter::start_ticker(&engine, interval)?;
        Ok(Arc::new(Self {
            engine,
            _ticker: ticker,
        }))
    }

    fn linker<S, C>(&self) -> Result<Linker<WasmBlockState<S, C>>>
    where
        S: Reader + Writer + Send + 'static,
        C: Codec + Send + Sync + 'static,
    {
        let mut linker = Linker::<WasmBlockState<S, C>>::new(&self.engine);
        BlockWorld::add_to_linker::<
            WasmBlockState<S, C>,
            wasmtime::component::HasSelf<WasmBlockState<S, C>>,
        >(&mut linker, |state: &mut WasmBlockState<S, C>| state)
        .map_err(wasm("linker"))?;
        Ok(linker)
    }

    /// Compile a component and retrieve its manifest once. Compilation is
    /// synchronous; call this from a blocking context for large artifacts.
    ///
    /// The manifest is a JSON blob declaring the block's name, version,
    /// serialization format, and path interface. The runtime reads it
    /// **before wiring** to discover what codec the block speaks — the store
    /// bridge can't be set up without it (see
    /// [why-manifest](https://github.com/StructFS/structfs/blob/main/isotope/rationale/04-why-manifest.md)).
    pub fn prepare(self: &Arc<Self>, bytes: &[u8]) -> Result<PreparedComponent> {
        let component = Component::new(&self.engine, bytes).map_err(wasm("component"))?;
        let linker = self.linker::<NoOpStore, NoCodec>()?;
        let state = WasmBlockState::new(NoOpStore, NoCodec, Format::OCTET_STREAM);
        let mut store = Store::new(&self.engine, state);
        store.set_fuel(MANIFEST_FUEL).map_err(wasm("fuel"))?;
        store.set_epoch_deadline(u64::MAX / 2);
        let instance = BlockWorld::instantiate(&mut store, &component, &linker)
            .map_err(wasm("instantiate"))?;
        let manifest = instance
            .featherweight_block_block()
            .call_manifest(&mut store)
            .map_err(wasm("manifest"))?;
        Ok(PreparedComponent {
            engine: self.clone(),
            component,
            manifest,
        })
    }
}

/// A compiled component and its manifest, bound to the engine that compiled
/// it. Every run gets a fresh store.
#[derive(Clone)]
pub struct PreparedComponent {
    engine: Arc<ComponentEngine>,
    component: Component,
    manifest: Vec<u8>,
}

impl PreparedComponent {
    /// The block's JSON manifest, captured at preparation.
    pub fn manifest(&self) -> &[u8] {
        &self.manifest
    }

    /// Run the guest's `run` export on the calling (blocking) thread over
    /// `root`, bridged with `codec` in `format`. `fuel` caps the run
    /// (`None` counts but does not cap); `cancel` interrupts it at the next
    /// epoch tick. Reports consumed fuel to `usage`.
    fn run<S, C>(
        &self,
        root: S,
        codec: C,
        format: Format,
        fuel: Option<u64>,
        cancel: CancelToken,
        usage: &featherweight_runtime::ExecutionMeter,
    ) -> Result<i32>
    where
        S: Reader + Writer + Send + 'static,
        C: Codec + Send + Sync + 'static,
    {
        let linker = self.engine.linker::<S, C>()?;
        let state = WasmBlockState::new(root, codec, format);
        let mut store = Store::new(&self.engine.engine, state);
        usage.configure_wasm(fuel, None);
        adapter::arm_store(&mut store, fuel, false, move || {
            if cancel.is_cancelled() {
                Err("guest interrupted: immediate shutdown".to_string())
            } else {
                Ok(())
            }
        })?;
        let result = BlockWorld::instantiate(&mut store, &self.component, &linker)
            .map_err(wasm("instantiate"))
            .and_then(|instance| {
                instance
                    .featherweight_block_block()
                    .call_run(&mut store)
                    .map_err(wasm("run"))
            });
        usage.sample_fuel(
            fuel.unwrap_or(u64::MAX)
                .saturating_sub(store.get_fuel().unwrap_or(0)),
        );
        match result? {
            Ok(()) => Ok(0),
            Err(message) => Err(RuntimeError::wasm("run", message)),
        }
    }
}

/// [`WasmBlockDriver`] over a prepared component: runs it with the standard
/// transports in the manifest-declared format.
struct ComponentDriver(PreparedComponent);

impl WasmBlockDriver for ComponentDriver {
    fn manifest(&self) -> Result<Vec<u8>> {
        Ok(self.0.manifest.clone())
    }

    fn execute(
        self: Arc<Self>,
        context: DriverContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<i32>> + Send>> {
        Box::pin(async move {
            // Component calls are synchronous: the run owns a blocking worker,
            // and joining it is required even after cancellation — the
            // namespace and the component store stay owned until it returns.
            let task = tokio::task::spawn_blocking(move || {
                self.0.run(
                    context.namespace,
                    MultiCodec::standard(),
                    context.format,
                    context.metering.fuel,
                    context.cancel,
                    &context.usage,
                )
            });
            task.await
                .map_err(|e| RuntimeError::task_failed("component task", e))?
        })
    }
}

/// The adapter's loader: claims wasm component artifacts (layer 1). Every
/// component it loads shares one engine.
#[derive(Default)]
pub struct ComponentLoader {
    engine: OnceLock<Arc<ComponentEngine>>,
}

impl ComponentLoader {
    /// A loader that creates its engine on first use.
    pub fn new() -> Self {
        Self::default()
    }

    /// A loader over an existing engine (shared across runtimes).
    pub fn with_engine(engine: Arc<ComponentEngine>) -> Self {
        let loader = Self::default();
        let _ = loader.engine.set(engine);
        loader
    }

    fn engine(&self) -> Result<Arc<ComponentEngine>> {
        if let Some(engine) = self.engine.get() {
            return Ok(engine.clone());
        }
        let engine = ComponentEngine::new()?;
        Ok(self.engine.get_or_init(|| engine).clone())
    }
}

impl ArtifactLoader for ComponentLoader {
    fn matches(&self, bytes: &[u8]) -> bool {
        is_component(bytes)
    }

    fn load(&self, bytes: Vec<u8>) -> Result<Arc<dyn WasmBlockDriver>> {
        Ok(Arc::new(ComponentDriver(self.engine()?.prepare(&bytes)?)))
    }
}

/// Teach a runtime configuration to load wasm components as blocks.
pub fn register(config: &mut RuntimeConfig) {
    config.register_loader(Arc::new(ComponentLoader::new()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::{Format, NoCodec, Path, Record};

    /// A store whose read/write echo fixed data — enough to exercise
    /// the CoreToLL bridge from the WIT side.
    struct TestStore;
    impl Reader for TestStore {
        fn read(
            &mut self,
            _path: &Path,
        ) -> std::result::Result<Option<Record>, structfs_core_store::Error> {
            Ok(None)
        }
    }
    impl Writer for TestStore {
        fn write(
            &mut self,
            path: &Path,
            _record: Record,
        ) -> std::result::Result<Path, structfs_core_store::Error> {
            Ok(path.clone())
        }
    }

    #[test]
    fn wasm_block_state_host_read_not_found() {
        use featherweight::block::ll_store::Host;
        let mut state = WasmBlockState::new(TestStore, NoCodec, Format::OCTET_STREAM);
        let result = state.read(vec![b"some".to_vec(), b"path".to_vec()]);
        assert_eq!(result, Ok(None));
    }

    #[test]
    fn wasm_block_state_host_read_found() {
        struct ValueStore;
        impl Reader for ValueStore {
            fn read(
                &mut self,
                _path: &Path,
            ) -> std::result::Result<Option<Record>, structfs_core_store::Error> {
                Ok(Some(Record::raw(
                    Bytes::from_static(b"test value"),
                    Format::OCTET_STREAM,
                )))
            }
        }
        impl Writer for ValueStore {
            fn write(
                &mut self,
                path: &Path,
                _record: Record,
            ) -> std::result::Result<Path, structfs_core_store::Error> {
                Ok(path.clone())
            }
        }

        use featherweight::block::ll_store::Host;
        let mut state = WasmBlockState::new(ValueStore, NoCodec, Format::OCTET_STREAM);
        let result = state.read(vec![b"some".to_vec(), b"path".to_vec()]);
        assert_eq!(result, Ok(Some(b"test value".to_vec())));
    }

    #[test]
    fn wasm_block_state_host_read_invalid_path() {
        use featherweight::block::ll_store::Host;
        let mut state = WasmBlockState::new(TestStore, NoCodec, Format::OCTET_STREAM);
        // Path with hyphen is invalid
        let result = state.read(vec![b"foo".to_vec(), b"bar-baz".to_vec()]);
        assert!(result.is_err());
    }

    #[test]
    fn wasm_block_state_host_write_success() {
        use featherweight::block::ll_store::Host;
        let mut state = WasmBlockState::new(TestStore, NoCodec, Format::OCTET_STREAM);
        let result = state.write(
            vec![b"output".to_vec(), b"test".to_vec()],
            b"hello".to_vec(),
        );
        assert_eq!(result, Ok(vec![b"output".to_vec(), b"test".to_vec()]));
    }

    #[test]
    fn wasm_block_state_host_write_invalid_path() {
        use featherweight::block::ll_store::Host;
        let mut state = WasmBlockState::new(TestStore, NoCodec, Format::OCTET_STREAM);
        // Path with hyphen is invalid
        let result = state.write(vec![b"foo".to_vec(), b"bar-baz".to_vec()], b"data".to_vec());
        assert!(result.is_err());
    }

    /// A store that panicked mid-operation poisons the bridge's mutex; the
    /// next host call still proceeds instead of panicking the guest's host.
    #[test]
    fn a_poisoned_bridge_recovers() {
        use featherweight::block::ll_store::Host;
        let mut state = WasmBlockState::new(TestStore, NoCodec, Format::OCTET_STREAM);
        let root = state.root.clone();
        let _ = std::thread::spawn(move || {
            let _guard = root.lock().unwrap();
            panic!("poison the bridge");
        })
        .join();
        assert!(state.root.is_poisoned());
        assert_eq!(state.read(vec![b"x".to_vec()]), Ok(None));
    }

    #[test]
    fn transports_cross_the_wit_bridge() {
        use std::collections::BTreeMap;
        use structfs_core_store::Value;
        use structfs_serde_store::CborCodec;

        /// Serves one record; remembers what was written (the bridge
        /// may hand it raw bytes tagged with the wire format).
        struct KvStore(Option<Record>);
        impl Reader for KvStore {
            fn read(
                &mut self,
                _path: &Path,
            ) -> std::result::Result<Option<Record>, structfs_core_store::Error> {
                Ok(self.0.clone())
            }
        }
        impl Writer for KvStore {
            fn write(
                &mut self,
                path: &Path,
                record: Record,
            ) -> std::result::Result<Path, structfs_core_store::Error> {
                self.0 = Some(record);
                Ok(path.clone())
            }
        }

        // The value contains real bytes — the fidelity the binary
        // transports carry through the ll-store boundary.
        let value = Value::Map(BTreeMap::from([(
            "payload".to_string(),
            Value::Bytes(vec![0, 159, 146, 150]),
        )]));

        use featherweight::block::ll_store::Host;
        let mut state = WasmBlockState::new(
            KvStore(Some(Record::parsed(value.clone()))),
            CborCodec,
            Format::CBOR,
        );

        // Guest-side read: the bridge encodes the parsed value as CBOR.
        let wire = state.read(vec![b"input".to_vec()]).unwrap().unwrap();
        let decoded: Value = ciborium_decode(&wire);
        assert_eq!(decoded, value);

        // Guest-side write: the CBOR bytes decode into the store as the
        // same parsed value, which the next read re-encodes.
        state.write(vec![b"output".to_vec()], wire).unwrap();
        let round = state.read(vec![b"output".to_vec()]).unwrap().unwrap();
        assert_eq!(ciborium_decode(&round), value);
    }

    fn ciborium_decode(bytes: &[u8]) -> structfs_core_store::Value {
        use structfs_core_store::Codec as _;
        structfs_serde_store::CborCodec
            .decode(&Bytes::copy_from_slice(bytes), &Format::CBOR)
            .unwrap()
    }

    #[tokio::test]
    async fn preparation_rejects_non_components_with_a_typed_engine_error() {
        let engine = ComponentEngine::new().unwrap();
        let err = engine
            .prepare(&[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00])
            .err()
            .expect("a core module is not a component");
        assert!(
            matches!(
                err,
                RuntimeError::Wasm {
                    operation: "component",
                    ..
                }
            ),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_loader_shares_one_engine() {
        let loader = ComponentLoader::new();
        let first = loader.engine().unwrap();
        let second = loader.engine().unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        let shared = ComponentLoader::with_engine(first.clone());
        assert!(Arc::ptr_eq(&shared.engine().unwrap(), &first));
    }

    #[test]
    fn engines_need_a_tokio_runtime() {
        assert!(matches!(
            ComponentEngine::new(),
            Err(RuntimeError::EngineConfig(_))
        ));
    }

    #[test]
    fn loader_claims_components_only() {
        let loader = ComponentLoader::new();
        // Component: version 0x0d, layer 1.
        assert!(loader.matches(b"\0asm\x0d\x00\x01\x00rest"));
        // Core module: version 1, layer 0 — the runtime's own binding.
        assert!(!loader.matches(b"\0asm\x01\x00\x00\x00rest"));
        assert!(!loader.matches(b"(module)"));
    }

    #[test]
    fn wasm_block_state_host_read_store_error() {
        struct FailingStore;
        impl Reader for FailingStore {
            fn read(
                &mut self,
                _path: &Path,
            ) -> std::result::Result<Option<Record>, structfs_core_store::Error> {
                Err(structfs_core_store::Error::store(
                    "test",
                    "read",
                    "test error",
                ))
            }
        }
        impl Writer for FailingStore {
            fn write(
                &mut self,
                path: &Path,
                _record: Record,
            ) -> std::result::Result<Path, structfs_core_store::Error> {
                Ok(path.clone())
            }
        }

        use featherweight::block::ll_store::Host;
        let mut state = WasmBlockState::new(FailingStore, NoCodec, Format::OCTET_STREAM);
        let result = state.read(vec![b"some".to_vec(), b"path".to_vec()]);
        assert!(result.is_err());
    }

    #[test]
    fn wasm_block_state_host_write_store_error() {
        struct FailingStore;
        impl Reader for FailingStore {
            fn read(
                &mut self,
                _path: &Path,
            ) -> std::result::Result<Option<Record>, structfs_core_store::Error> {
                Ok(None)
            }
        }
        impl Writer for FailingStore {
            fn write(
                &mut self,
                _path: &Path,
                _record: Record,
            ) -> std::result::Result<Path, structfs_core_store::Error> {
                Err(structfs_core_store::Error::store(
                    "test",
                    "write",
                    "test error",
                ))
            }
        }

        use featherweight::block::ll_store::Host;
        let mut state = WasmBlockState::new(FailingStore, NoCodec, Format::OCTET_STREAM);
        let result = state.write(
            vec![b"output".to_vec(), b"test".to_vec()],
            b"hello".to_vec(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn wasm_block_state_host_read_codec_error() {
        struct RawBytesStore;
        impl Reader for RawBytesStore {
            fn read(
                &mut self,
                _path: &Path,
            ) -> std::result::Result<Option<Record>, structfs_core_store::Error> {
                // Raw bytes in a format NoCodec can't transcode.
                Ok(Some(Record::raw(
                    Bytes::from_static(b"\xff\xfe"),
                    Format::JSON,
                )))
            }
        }
        impl Writer for RawBytesStore {
            fn write(
                &mut self,
                path: &Path,
                _record: Record,
            ) -> std::result::Result<Path, structfs_core_store::Error> {
                Ok(path.clone())
            }
        }

        use featherweight::block::ll_store::Host;
        let mut state = WasmBlockState::new(RawBytesStore, NoCodec, Format::OCTET_STREAM);
        let result = state.read(vec![b"some".to_vec(), b"path".to_vec()]);
        assert!(result.is_err());
    }
}
