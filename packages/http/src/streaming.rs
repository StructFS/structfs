//! Incremental native HTTP. No per-response producer task or whole-body buffer.
//!
//! The counterpart to the blocking [`crate::executor::BlockingHttpExecutor`]
//! seam: [`AsyncHttpExecutor`] hands back status and headers as soon as they
//! arrive, with the body still on the wire behind a pull-based
//! [`ByteStream`].
//!
//! No `reqwest` or `http` type appears in a public signature here: the
//! response is described with the same `u16` status and `HashMap` headers as
//! [`crate::HttpResponse`], so the streaming and buffered paths read alike
//! and neither pins a transport crate into this crate's API. Transport
//! failures are mapped through [`crate::Error`] so the typed error table in
//! [`crate::error`] applies — a timeout arrives as `DeadlineExceeded`.

use std::collections::HashMap;
use std::{future::Future, pin::Pin, time::Duration};

use structfs_core_store::{DetachedFuture, Error};

use crate::HttpRequest;

pub type ChunkFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<bytes::Bytes>, Error>> + Send + 'a>>;

/// Pull-based body. Dropping it abandons the response transport; no application
/// network task remains to join. Implementations must document additional work.
pub trait ByteStream: Send {
    fn next_chunk(&mut self) -> ChunkFuture<'_>;
}

/// A response whose head has arrived and whose body has not.
///
/// `status` and `headers` mirror [`crate::HttpResponse`]; header values that
/// are not valid UTF-8 are dropped, as they are there.
///
/// Headers are a single-valued map: a header repeated in the response
/// (`Set-Cookie`, `Via`, …) collapses to the **last** value received.
#[non_exhaustive]
pub struct StreamingResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response headers, lowercased as the transport delivered them.
    pub headers: HashMap<String, String>,
    /// The body, still being received.
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

/// The async execution seam: status and headers first, body on demand.
pub trait AsyncHttpExecutor: Send + Sync {
    fn execute(&self, request: HttpRequest) -> DetachedFuture<StreamingResponse>;
}

/// Production async executor over one pooled `reqwest::Client`.
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

/// Map a transport failure through the crate's typed error table.
fn transport(error: reqwest::Error) -> Error {
    crate::Error::Http(error).into()
}

/// The body of a `reqwest` response, as a [`ByteStream`].
///
/// Private so that `reqwest::Response` never reaches a public signature.
struct ReqwestBody(reqwest::Response);

impl ByteStream for ReqwestBody {
    fn next_chunk(&mut self) -> ChunkFuture<'_> {
        Box::pin(async { self.0.chunk().await.map_err(transport) })
    }
}

impl AsyncHttpExecutor for AsyncReqwestExecutor {
    fn execute(&self, request: HttpRequest) -> DetachedFuture<StreamingResponse> {
        let client = self.0.clone();
        Box::pin(async move {
            // Malformed or non-http(s) URLs are InvalidArgument, not transport.
            let url = request.url().map_err(Error::from)?;
            let mut builder = client
                .request(request.method.into(), url)
                .query(&request.query);
            for (name, value) in request.headers {
                builder = builder.header(name, value);
            }
            if let Some(body) = request.body {
                builder = builder.json(&body);
            }
            let response = builder.send().await.map_err(transport)?;
            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .to_str()
                        .ok()
                        .map(|v| (name.to_string(), v.to_string()))
                })
                .collect();
            Ok(StreamingResponse {
                status,
                headers,
                body: Box::new(ReqwestBody(response)),
            })
        })
    }
}
