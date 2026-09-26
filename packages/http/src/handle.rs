use serde::{Deserialize, Serialize};
use structfs_core_store::Reference;

/// The state of an async HTTP request
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum RequestState {
    /// Request is in progress
    Pending,
    /// Request completed successfully
    Complete,
    /// Request failed with an error
    Failed,
}

/// Status of an async HTTP request handle
///
/// Read this from a handle path (e.g., `outstanding/{id}`) to check request status.
/// Uses [`Reference`] for HATEOAS-compliant navigation; its serde form is the
/// canonical `{"path": …, "type": {"name": …}}` map, the same shape
/// `Reference::to_value` produces.
///
/// Construct with [`RequestStatus::pending`], [`RequestStatus::complete`] or
/// [`RequestStatus::failed`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RequestStatus {
    /// Current state of the request
    pub state: RequestState,

    /// Error message if state is Failed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Reference to the original request
    pub request: Reference,

    /// Reference to the response (available when Complete)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<Reference>,
}

fn request_reference(id: &str) -> Reference {
    Reference::with_type(format!("outstanding/{}/request", id), "http-request")
}

fn response_reference(id: &str) -> Reference {
    Reference::with_type(format!("outstanding/{}/response", id), "http-response")
}

impl RequestStatus {
    pub fn pending(id: String) -> Self {
        Self {
            state: RequestState::Pending,
            error: None,
            request: request_reference(&id),
            response: None,
        }
    }

    pub fn complete(id: String) -> Self {
        Self {
            state: RequestState::Complete,
            error: None,
            request: request_reference(&id),
            response: Some(response_reference(&id)),
        }
    }

    pub fn failed(id: String, error: String) -> Self {
        Self {
            state: RequestState::Failed,
            error: Some(error),
            request: request_reference(&id),
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

    /// The wire form must be byte-identical to what the deleted
    /// `SerializableReference`/`SerializableTypeInfo` pair produced, so
    /// existing handle readers keep parsing.
    #[test]
    fn request_status_wire_form_is_unchanged() {
        assert_eq!(
            serde_json::to_string(&RequestStatus::pending("0".into())).unwrap(),
            r#"{"state":"pending","request":{"path":"outstanding/0/request","type":{"name":"http-request"}}}"#
        );
        assert_eq!(
            serde_json::to_string(&RequestStatus::complete("0".into())).unwrap(),
            r#"{"state":"complete","request":{"path":"outstanding/0/request","type":{"name":"http-request"}},"response":{"path":"outstanding/0/response","type":{"name":"http-response"}}}"#
        );
        assert_eq!(
            serde_json::to_string(&RequestStatus::failed("0".into(), "boom".into())).unwrap(),
            r#"{"state":"failed","error":"boom","request":{"path":"outstanding/0/request","type":{"name":"http-request"}}}"#
        );
    }

    #[test]
    fn request_status_round_trips() {
        let status = RequestStatus::complete("7".into());
        let json = serde_json::to_string(&status).unwrap();
        let parsed: RequestStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.request, status.request);
        assert_eq!(parsed.response, status.response);
        assert_eq!(parsed.state, status.state);
    }
}
