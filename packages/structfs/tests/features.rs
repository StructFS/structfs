//! Every facade path, per feature. Each test is compiled only under the
//! feature it exercises, so `cargo test -p structfs --features <f>` and
//! `--all-features` pin the documented layout; a moved or renamed item breaks
//! the build here first.

use structfs::{path, MemoryStore, Reader, Record, Value, Writer};

#[test]
fn core_vocabulary_is_at_the_root() {
    let mut store = MemoryStore::new();
    store
        .write(&path!("a/b"), Record::parsed(Value::from("x")))
        .unwrap();
    assert!(store.read(&path!("a/b")).unwrap().is_some());
    structfs::conformance::check_conventions(&mut MemoryStore::new());

    let _: fn(MemoryStore) -> structfs::ReadOnly<MemoryStore> = structfs::ReadOnly::new;
    let _: Option<structfs::OverlayStore> = None;
    let _: Option<structfs::PathTrie<u8>> = None;
    let _: Option<structfs::Bytes> = None;
}

#[test]
fn byte_layer_and_patterns_are_modules() {
    let _: Option<structfs::ll::LLPath> = None;
    let _: Option<structfs::ll::LLError> = None;
    let _: Option<structfs::ll::CoreToLL<MemoryStore, structfs::NoCodec>> = None;
    fn _bounds<T: structfs::ll::LLReader + structfs::ll::LLWriter + structfs::ll::LLStore>() {}

    let p = path!("users/alice/profile");
    assert!(structfs::pattern::matches_prefix_suffix(
        &p,
        &path!("users"),
        &path!("profile"),
        1
    ));
    let _: Option<structfs::pattern::PathPattern> = None;
}

#[cfg(feature = "async")]
#[test]
fn async_traits_are_at_the_root() {
    fn _bounds<T: structfs::AsyncReader + structfs::DetachedReader + structfs::SharedReader>() {}
    let _: Option<structfs::ll::SyncToAsyncLL<()>> = None;
    let shared: std::sync::Arc<dyn structfs::SharedWriter> = std::sync::Arc::new(
        structfs::DetachedShared::new(structfs::Shared::new(MemoryStore::new())),
    );
    drop(shared);
}

#[cfg(feature = "typed")]
#[test]
fn typed_module() {
    use structfs::typed::{TypedReader, TypedWriter};

    let mut store = MemoryStore::new();
    store.write_typed(&path!("n"), &7u32).unwrap();
    assert_eq!(store.read_typed::<u32>(&path!("n")).unwrap(), Some(7));

    let _ = structfs::typed::to_value(&1u8).unwrap();
    let _: Option<structfs::typed::CodecProfile> = None;
    let _: Option<structfs::typed::Limits> = None;
    let _ = (
        structfs::typed::JsonCodec,
        structfs::typed::ValueJsonCodec,
        structfs::typed::CborCodec,
        structfs::typed::FlexbuffersCodec,
    );
    let _: Option<structfs::typed::MultiCodec> = None;
    let _: Option<structfs::typed::ValueCodec> = None;
}

#[cfg(all(feature = "typed", feature = "async"))]
#[test]
fn typed_async_helpers() {
    fn _bounds<
        T: structfs::typed::AsyncTypedReader
            + structfs::typed::AsyncTypedWriter
            + structfs::typed::DetachedTypedReader
            + structfs::typed::DetachedTypedWriter,
    >() {
    }
}

#[cfg(feature = "persist")]
#[test]
fn persist_module() {
    use structfs::persist::{LogStore, MemoryAppendBacking};

    let _: Option<structfs::persist::BackedStore<structfs::persist::JsonFileBacking>> = None;
    let _: Option<structfs::persist::JsonlFileBacking> = None;
    let _ = structfs::persist::Durability::Synced;
    fn _bounds<T: structfs::persist::Backing + structfs::persist::AppendBacking>() {}

    let _: Option<LogStore<MemoryAppendBacking>> = None;
}

#[cfg(feature = "net")]
#[test]
fn net_schema_is_portable() {
    let request = structfs::net::HttpRequest::get("https://example.com");
    assert_eq!(request.method, structfs::net::Method::GET);
    let _: Option<structfs::net::HttpResponse> = None;
    let _: Option<structfs::net::RequestStatus> = None;
    let _: Option<structfs::net::RequestState> = None;
    let _: Option<structfs::net::Error> = None;
    let _: Option<structfs::net::sse::SseFramer> = None;
}

#[cfg(feature = "net-blocking")]
#[test]
fn net_blocking_stores() {
    let _: Option<structfs::net::HttpBrokerStore> = None;
    let _: Option<structfs::net::BackgroundHttpBrokerStore> = None;
    let _: Option<structfs::net::HttpClientStore> = None;
    let _: Option<structfs::net::BlockingReqwestExecutor> = None;
    fn _bounds<T: structfs::net::BlockingHttpExecutor>() {}
}

#[cfg(feature = "net-streaming")]
#[test]
fn net_streaming_executor() {
    let _: Option<structfs::net::streaming::AsyncReqwestExecutor> = None;
    fn _bounds<T: structfs::net::streaming::AsyncHttpExecutor>() {}
}

#[cfg(feature = "os")]
#[test]
fn os_module() {
    let mut sys = structfs::os::SysStore::new();
    assert!(sys.read(&path!("time/now")).unwrap().is_some());
    let _: Option<structfs::os::FsStore> = None;
    let _ = structfs::os::MAX_SLEEP;
}

#[cfg(feature = "handles")]
#[test]
fn handles_module() {
    let token = structfs::handles::CancelToken::new();
    token.cancel();
    assert!(token.is_cancelled());
    let _: Option<structfs::handles::Gate> = None;
    let _: Option<structfs::handles::DuplexStream> = None;
    fn _bounds<T: structfs::handles::HandleProtocol>() {}
}

#[cfg(feature = "service")]
#[test]
fn service_module() {
    let _: Option<structfs::service::CleanupSupervisor> = None;
}

#[cfg(feature = "state")]
#[test]
fn state_module() {
    let _: Option<structfs::state::State> = None;
}

#[cfg(feature = "profiles")]
#[test]
fn profiles_module() {
    let _: Option<structfs::profiles::HeadlessHost> = None;
}
