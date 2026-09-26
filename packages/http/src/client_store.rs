//! Direct HTTP client store: read is GET, write is POST.

use collection_literals::btree;

use structfs_core_store::{Error, NoCodec, Path, Reader, Record, Value, Writer};
use structfs_serde_store::to_value;

use crate::broker_common::DOCS_PATH;
use crate::executor::{BlockingHttpExecutor, BlockingReqwestExecutor};
use crate::types::{HttpRequest, HttpResponse, Method};

/// Generate documentation for the HTTP client store.
fn http_client_docs() -> Value {
    Value::Map(btree! {
        "title".into() => Value::String("HTTP Client Store".into()),
        "description".into() => Value::String("Direct HTTP client with a base URL. Read = GET, Write = POST.".into()),
        "paths".into() => Value::Map(btree! {
            "read /<path>".into() => Value::String("GET request to base_url/<path>".into()),
            "write /<path> <json>".into() => Value::String("POST request to base_url/<path>".into()),
            "write / <HttpRequest>".into() => Value::String("Execute arbitrary request".into()),
            "write / <other json>".into() => Value::String("POST the value as the body of base_url; a map with a `method` or `path` key is treated as an HttpRequest instead".into()),
        }),
        "example".into() => Value::Array(vec![
            Value::String("# Mount at /api with base URL".into()),
            Value::String("write /ctx/mounts/api {\"type\": \"http\", \"url\": \"https://api.example.com\"}".into()),
            Value::String("read /api/users  # GET https://api.example.com/users".into()),
            Value::String("write /api/users {\"name\": \"Alice\"}  # POST with body".into()),
        ]),
    })
}

/// HTTP client store for direct requests.
///
/// Maps read/write operations to GET/POST requests against a base URL.
/// Generic over the HTTP executor to allow mocking in tests.
///
/// # Root writes
///
/// `write /` accepts *either* a full [`HttpRequest`] or an arbitrary value
/// to POST as the body, told apart structurally:
///
/// - A map with a `method` or `path` key **is a request**. It must then
///   parse as an [`HttpRequest`] (strictly: unknown fields, a lowercase or
///   unknown method, or a wrongly typed field are all rejected) or the write
///   fails with `InvalidArgument` — a typo'd request is never POSTed as a
///   body. A missing `method` defaults to `GET`, so `{"path": "x"}` is
///   `GET <base>/x`.
/// - Anything else is POSTed to the base URL as the body: `{"name": "Bob"}`,
///   scalars, arrays, and the empty map `{}` (which POSTs `{}`).
///
/// To POST a body that itself has a `method` or `path` field to the base
/// URL, wrap it in an explicit request:
/// `{"method": "POST", "body": {"path": "/a", "name": "x"}}`.
///
/// Previously every map parsed as a request, so `{"name": "Bob"}` silently
/// sent `GET <base>`.
///
/// # Runtime constraint
///
/// The default executor builds a blocking reqwest client, which **panics
/// when constructed inside a Tokio runtime**. Build this store on an
/// ordinary thread, or supply your own executor with
/// [`HttpClientStore::with_executor`].
///
/// # Examples
///
/// ```no_run
/// use structfs_core_store::{path, Reader};
/// use structfs_http::HttpClientStore;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let mut client = HttpClientStore::new("https://api.example.com")?;
///
/// // GET https://api.example.com/users/123
/// let record = client.read(&path!("users/123"))?;
/// # Ok(())
/// # }
/// ```
pub struct HttpClientStore<E: BlockingHttpExecutor = BlockingReqwestExecutor> {
    executor: E,
    base_url: url::Url,
    default_headers: std::collections::HashMap<String, String>,
}

impl HttpClientStore<BlockingReqwestExecutor> {
    /// Create a new HTTP client store with the given base URL.
    ///
    /// # Panics
    ///
    /// Panics if called from within a Tokio runtime; see the type docs.
    pub fn new(base_url: &str) -> Result<Self, crate::Error> {
        let base_url = url::Url::parse(base_url)?;
        let executor = BlockingReqwestExecutor::with_default_timeout()?;

        Ok(Self {
            executor,
            base_url,
            default_headers: std::collections::HashMap::new(),
        })
    }
}

impl<E: BlockingHttpExecutor> HttpClientStore<E> {
    /// Create a new HTTP client store with a custom executor.
    ///
    /// This is also how tests substitute a mock executor.
    pub fn with_executor(base_url: &str, executor: E) -> Result<Self, crate::Error> {
        let base_url = url::Url::parse(base_url)?;

        Ok(Self {
            executor,
            base_url,
            default_headers: std::collections::HashMap::new(),
        })
    }

    /// Add a default header that will be sent with every request
    #[must_use]
    pub fn with_default_header(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.default_headers.insert(name.into(), value.into());
        self
    }

    /// Build a full request with base URL, default headers, etc.
    fn build_request(&self, mut request: HttpRequest) -> HttpRequest {
        // Resolve relative URLs against base URL
        if !request.path.starts_with("http://") && !request.path.starts_with("https://") {
            if let Ok(url) = self.base_url.join(&request.path) {
                request.path = url.to_string();
            }
        }

        // Add default headers (request headers take precedence)
        for (name, value) in &self.default_headers {
            if !request.headers.contains_key(name) {
                request.headers.insert(name.clone(), value.clone());
            }
        }

        request
    }

    /// Perform a GET request and return the response
    pub fn get(&self, path: &Path) -> Result<HttpResponse, crate::Error> {
        let full_request = self.build_request(HttpRequest::new(Method::GET, path.to_string()));
        self.executor.execute(&full_request)
    }

    /// Execute a request after resolving it against the base URL.
    fn execute(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
        let full_request = self.build_request(request);
        Ok(self.executor.execute(&full_request)?)
    }

    /// POST `value` as a JSON body to `path`.
    fn post_value(&self, path: String, value: Value) -> Result<HttpResponse, Error> {
        let json_value = structfs_serde_store::value_to_json(value)?;
        let mut request = HttpRequest::new(Method::POST, path);
        request.body = Some(json_value);
        self.execute(request)
    }
}

/// Whether a root write is a request specification rather than a body.
///
/// The rule is structural and deliberately simple: a map with a `method` or
/// `path` key is a request. Everything else — including the empty map `{}` —
/// is a value to POST.
fn is_request_spec(value: &Value) -> bool {
    matches!(value, Value::Map(map) if map.contains_key("method") || map.contains_key("path"))
}

/// Turn a non-success response into a typed error carrying the store path.
fn status_error(path: &Path, response: HttpResponse) -> Error {
    crate::Error::status_at(
        path.clone(),
        response.status,
        response.status_text,
        response.body_text.unwrap_or_default(),
    )
    .into()
}

impl<E: BlockingHttpExecutor> Reader for HttpClientStore<E> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        // Handle docs: read /docs or /docs/... -> documentation
        if !from.is_empty() && &from[0] == DOCS_PATH {
            return Ok(Some(Record::parsed(http_client_docs())));
        }

        let response = self.get(from)?;

        // A missing resource is `Ok(None)`, per the `Reader::read` contract.
        if response.status == 404 {
            return Ok(None);
        }

        if !response.is_success() {
            return Err(status_error(from, response));
        }

        // Convert response body to Value
        let value = to_value(&response.body)
            .map_err(|e| Error::encode(structfs_core_store::Format::JSON, e.to_string()))?;

        Ok(Some(Record::parsed(value)))
    }
}

impl<E: BlockingHttpExecutor> Writer for HttpClientStore<E> {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        let value = data.into_value(&NoCodec)?;

        let response = if to.is_empty() {
            if is_request_spec(&value) {
                // It claims to be a request; a malformed one is the
                // caller's mistake, never a body to POST.
                let request: HttpRequest =
                    structfs_serde_store::from_value(value).map_err(|e| {
                        Error::invalid_argument(format!(
                            "root write names `method` or `path` but is not a valid HttpRequest: {e}"
                        ))
                    })?;
                self.execute(request)?
            } else {
                self.post_value(String::new(), value)?
            }
        } else {
            self.post_value(to.to_string(), value)?
        };

        if !response.is_success() {
            return Err(status_error(to, response));
        }

        Ok(to.clone())
    }
}

#[cfg(test)]
#[path = "client_store_tests.rs"]
mod tests;
