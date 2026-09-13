//! Browser-shared application schema and cancellation, built independently of
//! native workspace dependencies. Native hosts opt into their executor features.
use structfs::{DetachedTypedReader, DetachedTypedWriter};
use structfs_core_store::{path, DetachedFuture, MemoryStore, Path, Shared};
use structfs_handles::CancelToken;
use structfs_http::{HttpRequest, HttpResponse};

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Settings {
    #[serde(with = "structfs_core_store::path_serde::components")]
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
