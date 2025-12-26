use structfs_core_store::Error as CoreError;

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
    Store(#[from] CoreError),
}

impl From<Error> for CoreError {
    fn from(error: Error) -> Self {
        CoreError::Other {
            message: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_url_error_display() {
        let e = Error::InvalidUrl {
            message: "missing scheme".to_string(),
        };
        assert!(e.to_string().contains("missing scheme"));
    }

    #[test]
    fn invalid_method_error_display() {
        let e = Error::InvalidMethod {
            method: "FOOBAR".to_string(),
        };
        assert!(e.to_string().contains("FOOBAR"));
    }

    #[test]
    fn json_error_conversion() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let e = Error::from(json_err);
        assert!(e.to_string().contains("JSON error"));
    }

    #[test]
    fn store_error_conversion() {
        let store_err = CoreError::Other {
            message: "test error".to_string(),
        };
        let e = Error::from(store_err);
        assert!(e.to_string().contains("Store error"));
    }

    #[test]
    fn error_to_core_error() {
        let e = Error::InvalidUrl {
            message: "bad url".to_string(),
        };
        let core_err: CoreError = e.into();
        assert!(core_err.to_string().contains("bad url"));
    }

    #[test]
    fn url_parse_error_conversion() {
        let url_err = url::Url::parse("not a url").unwrap_err();
        let e = Error::from(url_err);
        assert!(e.to_string().contains("URL parse error"));
    }
}
