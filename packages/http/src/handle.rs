use serde::{Deserialize, Serialize};

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

/// Status of an async HTTP request handle
///
/// Read this from a handle path (e.g., `handles/{id}`) to check request status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestStatus {
    /// Unique identifier for this request
    pub id: String,

    /// Current state of the request
    pub state: RequestState,

    /// Error message if state is Failed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Path to read the response from (available when Complete)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_path: Option<String>,
}

impl RequestStatus {
    pub fn pending(id: String) -> Self {
        Self {
            id,
            state: RequestState::Pending,
            error: None,
            response_path: None,
        }
    }

    pub fn complete(id: String) -> Self {
        let response_path = format!("handles/{}/response", id);
        Self {
            id,
            state: RequestState::Complete,
            error: None,
            response_path: Some(response_path),
        }
    }

    pub fn failed(id: String, error: String) -> Self {
        Self {
            id,
            state: RequestState::Failed,
            error: Some(error),
            response_path: None,
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
        assert_eq!(status.id, "123");
        assert!(status.error.is_none());
        assert!(status.response_path.is_none());
    }

    #[test]
    fn request_status_complete() {
        let status = RequestStatus::complete("456".to_string());
        assert!(!status.is_pending());
        assert!(status.is_complete());
        assert!(!status.is_failed());
        assert_eq!(status.id, "456");
        assert!(status.error.is_none());
        assert_eq!(
            status.response_path,
            Some("handles/456/response".to_string())
        );
    }

    #[test]
    fn request_status_failed() {
        let status = RequestStatus::failed("789".to_string(), "connection refused".to_string());
        assert!(!status.is_pending());
        assert!(!status.is_complete());
        assert!(status.is_failed());
        assert_eq!(status.id, "789");
        assert_eq!(status.error, Some("connection refused".to_string()));
        assert!(status.response_path.is_none());
    }

    #[test]
    fn request_state_equality() {
        assert_eq!(RequestState::Pending, RequestState::Pending);
        assert_eq!(RequestState::Complete, RequestState::Complete);
        assert_eq!(RequestState::Failed, RequestState::Failed);
        assert_ne!(RequestState::Pending, RequestState::Complete);
    }
}
