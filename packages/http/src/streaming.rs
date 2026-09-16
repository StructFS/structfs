//! Incremental native HTTP. No per-response producer task or whole-body buffer.
use crate::HttpRequest;
use std::{future::Future, pin::Pin, time::Duration};
use structfs_core_store::{DetachedFuture, Error};

pub type ChunkFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<bytes::Bytes>, Error>> + Send + 'a>>;
/// Pull-based body. Dropping it abandons the response transport; no application
/// network task remains to join. Implementations must document additional work.
pub trait ByteStream: Send {
    fn next_chunk(&mut self) -> ChunkFuture<'_>;
}
pub struct StreamingResponse {
    pub status: http::StatusCode,
    pub headers: http::HeaderMap,
    pub body: Box<dyn ByteStream>,
}
impl StreamingResponse {
    /// Explicitly buffer a bounded body (for example a 64 KiB error response).
    /// On overflow or transport failure, the body is dropped immediately.
    pub async fn read_limited(mut self, max_bytes: usize) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::new();
        while let Some(chunk) = self.body.next_chunk().await? {
            if chunk.len() > max_bytes.saturating_sub(bytes.len()) {
                return Err(Error::resource_limit("HTTP body exceeds byte limit"));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}
pub trait AsyncHttpExecutor: Send + Sync {
    fn execute(&self, request: HttpRequest) -> DetachedFuture<StreamingResponse>;
}
#[derive(Clone)]
pub struct AsyncReqwestExecutor(reqwest::Client);
impl AsyncReqwestExecutor {
    /// Timeout covers response headers and body consumption.
    pub fn new(timeout: Duration) -> Result<Self, Error> {
        reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map(Self)
            .map_err(transport)
    }
}
fn transport(error: reqwest::Error) -> Error {
    Error::store("http", "stream", error.to_string())
}
impl ByteStream for reqwest::Response {
    fn next_chunk(&mut self) -> ChunkFuture<'_> {
        Box::pin(async { self.chunk().await.map_err(transport) })
    }
}
impl AsyncHttpExecutor for AsyncReqwestExecutor {
    fn execute(&self, request: HttpRequest) -> DetachedFuture<StreamingResponse> {
        let client = self.0.clone();
        Box::pin(async move {
            let mut builder = client
                .request(request.method.into(), &request.path)
                .query(&request.query);
            for (name, value) in request.headers {
                builder = builder.header(name, value);
            }
            if let Some(body) = request.body {
                builder = builder.json(&body);
            }
            let response = builder.send().await.map_err(transport)?;
            Ok(StreamingResponse {
                status: response.status(),
                headers: response.headers().clone(),
                body: Box::new(response),
            })
        })
    }
}
