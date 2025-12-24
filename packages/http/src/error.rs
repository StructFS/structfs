use structfs_store::Error as StoreError;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("URL parse error: {0}")]
    UrlParse(#[from] url::ParseError),

    #[error("Invalid URL: {message}")]
    InvalidUrl { message: String },

    #[error("Invalid HTTP method: {method}")]
    InvalidMethod { method: String },

    #[error("Invalid header name: {0}")]
    InvalidHeaderName(#[from] http::header::InvalidHeaderName),

    #[error("Invalid header value: {0}")]
    InvalidHeaderValue(#[from] http::header::InvalidHeaderValue),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Store error: {0}")]
    Store(#[from] StoreError),
}

impl From<Error> for StoreError {
    fn from(error: Error) -> Self {
        StoreError::ImplementationFailure {
            message: error.to_string(),
        }
    }
}
