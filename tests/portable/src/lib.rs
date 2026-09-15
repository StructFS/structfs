//! Browser-shared application schema and cancellation, built independently of
//! native workspace dependencies. Native hosts opt into their executor features.
use store_core::{path, DetachedFuture, MemoryStore, Path, Shared};
use structfs::{DetachedTypedReader, DetachedTypedWriter};
use structfs_handles::CancelToken;
use structfs_http::{HttpRequest, HttpResponse};

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Settings {
    #[serde(with = "store_core::path_serde::components")]
    pub path: Path,
    pub request: HttpRequest,
    pub response: Option<HttpResponse>,
}

pub fn independent_operations() -> (DetachedFuture<Path>, DetachedFuture<Option<i64>>) {
    let mut store = Shared::new(MemoryStore::new());
    let write = store.write_as_detached(&path!("setting"), &1i64);
    let read = store.read_typed_detached(&path!("setting"));
    (write, read)
}

pub async fn cancellation(cancel: CancelToken) {
    cancel.cancelled().await;
}

#[cfg(feature = "native")]
pub fn native_client() -> Result<structfs_http::HttpClientStore, structfs_http::Error> {
    structfs_http::HttpClientStore::new("https://example.com")
}

// Macro expansion follows its defining crate even through a renamed dependency
// and a facade. Neither invocation requires the old extern-crate name.
pub fn hygienic_paths() -> (Path, Path) {
    (store_core::path!("renamed"), structfs::path!("facade"))
}

pub fn shared_writer() -> std::sync::Arc<dyn store_core::SharedWriter> {
    std::sync::Arc::new(store_core::DetachedShared::new(Shared::new(
        MemoryStore::new(),
    )))
}
