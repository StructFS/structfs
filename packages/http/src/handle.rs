use serde::{Deserialize, Serialize};

use crate::types::HttpResponse;

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

/// Internal state for a request handle
#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct HandleState {
    pub status: RequestStatus,
    pub response: Option<HttpResponse>,
}

#[allow(dead_code)]
impl HandleState {
    pub fn new(id: String) -> Self {
        Self {
            status: RequestStatus::pending(id),
            response: None,
        }
    }

    pub fn complete(&mut self, response: HttpResponse) {
        self.status = RequestStatus::complete(self.status.id.clone());
        self.response = Some(response);
    }

    pub fn fail(&mut self, error: String) {
        self.status = RequestStatus::failed(self.status.id.clone(), error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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
    fn handle_state_new() {
        let state = HandleState::new("test".to_string());
        assert!(state.status.is_pending());
        assert!(state.response.is_none());
    }

    #[test]
    fn handle_state_complete() {
        let mut state = HandleState::new("test".to_string());
        let response = HttpResponse {
            status: 200,
            status_text: "OK".to_string(),
            headers: HashMap::new(),
            body: serde_json::json!({"result": "success"}),
            body_text: None,
        };
        state.complete(response.clone());

        assert!(state.status.is_complete());
        assert!(state.response.is_some());
        assert_eq!(state.response.unwrap().status, 200);
    }

    #[test]
    fn handle_state_fail() {
        let mut state = HandleState::new("test".to_string());
        state.fail("network error".to_string());

        assert!(state.status.is_failed());
        assert_eq!(state.status.error, Some("network error".to_string()));
        assert!(state.response.is_none());
    }

    #[test]
    fn request_state_equality() {
        assert_eq!(RequestState::Pending, RequestState::Pending);
        assert_eq!(RequestState::Complete, RequestState::Complete);
        assert_eq!(RequestState::Failed, RequestState::Failed);
        assert_ne!(RequestState::Pending, RequestState::Complete);
    }
}
