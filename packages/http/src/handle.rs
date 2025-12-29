use serde::{Deserialize, Serialize};
use structfs_core_store::Reference;

/// The state of an async HTTP request
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RequestState {
    /// Request is in progress
    Pending,
    /// Request completed successfully
    Complete,
    /// Request failed with an error
    Failed,
}

/// Serializable reference type for RequestStatus.
///
/// This wraps structfs_core_store::Reference with serde support.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableReference {
    /// The path to the referenced value.
    pub path: String,

    /// Optional type information.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_info: Option<SerializableTypeInfo>,
}

/// Serializable type info for references.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableTypeInfo {
    /// Type name (e.g., "http-request", "http-response").
    pub name: String,
}

impl SerializableReference {
    /// Create a reference with a type name.
    pub fn with_type(path: impl Into<String>, type_name: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            type_info: Some(SerializableTypeInfo {
                name: type_name.into(),
            }),
        }
    }
}

impl From<Reference> for SerializableReference {
    fn from(r: Reference) -> Self {
        Self {
            path: r.path,
            type_info: r.type_info.map(|ti| SerializableTypeInfo { name: ti.name }),
        }
    }
}

/// Status of an async HTTP request handle
///
/// Read this from a handle path (e.g., `outstanding/{id}`) to check request status.
/// Uses References for HATEOAS-compliant navigation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestStatus {
    /// Current state of the request
    pub state: RequestState,

    /// Error message if state is Failed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Reference to the original request
    pub request: SerializableReference,

    /// Reference to the response (available when Complete)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<SerializableReference>,
}

impl RequestStatus {
    pub fn pending(id: String) -> Self {
        Self {
            state: RequestState::Pending,
            error: None,
            request: SerializableReference::with_type(
                format!("outstanding/{}/request", id),
                "http-request",
            ),
            response: None,
        }
    }

    pub fn complete(id: String) -> Self {
        Self {
            state: RequestState::Complete,
            error: None,
            request: SerializableReference::with_type(
                format!("outstanding/{}/request", id),
                "http-request",
            ),
            response: Some(SerializableReference::with_type(
                format!("outstanding/{}/response", id),
                "http-response",
            )),
        }
    }

    pub fn failed(id: String, error: String) -> Self {
        Self {
            state: RequestState::Failed,
            error: Some(error),
            request: SerializableReference::with_type(
                format!("outstanding/{}/request", id),
                "http-request",
            ),
            response: None,
        }
    }

    pub fn is_pending(&self) -> bool {
        self.state == RequestState::Pending
    }

    pub fn is_complete(&self) -> bool {
        self.state == RequestState::Complete
    }

    pub fn is_failed(&self) -> bool {
        self.state == RequestState::Failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_status_pending() {
        let status = RequestStatus::pending("123".to_string());
        assert!(status.is_pending());
        assert!(!status.is_complete());
        assert!(!status.is_failed());
        assert_eq!(status.request.path, "outstanding/123/request");
        assert_eq!(
            status.request.type_info.as_ref().unwrap().name,
            "http-request"
        );
        assert!(status.error.is_none());
        assert!(status.response.is_none());
    }

    #[test]
    fn request_status_complete() {
        let status = RequestStatus::complete("456".to_string());
        assert!(!status.is_pending());
        assert!(status.is_complete());
        assert!(!status.is_failed());
        assert_eq!(status.request.path, "outstanding/456/request");
        assert!(status.error.is_none());
        let response = status.response.as_ref().unwrap();
        assert_eq!(response.path, "outstanding/456/response");
        assert_eq!(response.type_info.as_ref().unwrap().name, "http-response");
    }

    #[test]
    fn request_status_failed() {
        let status = RequestStatus::failed("789".to_string(), "connection refused".to_string());
        assert!(!status.is_pending());
        assert!(!status.is_complete());
        assert!(status.is_failed());
        assert_eq!(status.request.path, "outstanding/789/request");
        assert_eq!(status.error, Some("connection refused".to_string()));
        assert!(status.response.is_none());
    }

    #[test]
    fn request_state_equality() {
        assert_eq!(RequestState::Pending, RequestState::Pending);
        assert_eq!(RequestState::Complete, RequestState::Complete);
        assert_eq!(RequestState::Failed, RequestState::Failed);
        assert_ne!(RequestState::Pending, RequestState::Complete);
    }

    #[test]
    fn serializable_reference_with_type() {
        let r = SerializableReference::with_type("outstanding/0/response", "http-response");
        assert_eq!(r.path, "outstanding/0/response");
        assert_eq!(r.type_info.as_ref().unwrap().name, "http-response");
    }

    #[test]
    fn serializable_reference_from_reference() {
        let core_ref = Reference::with_type("handles/0", "handle");
        let ser_ref: SerializableReference = core_ref.into();
        assert_eq!(ser_ref.path, "handles/0");
        assert_eq!(ser_ref.type_info.as_ref().unwrap().name, "handle");
    }
}
