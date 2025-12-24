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
#[derive(Debug)]
pub(crate) struct HandleState {
    pub status: RequestStatus,
    pub response: Option<HttpResponse>,
}

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
