pub mod chat;
pub mod dashboard;
pub mod mcp;
pub mod provider;
pub mod widget;
pub mod workflow;

use serde::{Deserialize, Serialize};

/// Unique identifier type
pub type Id = String;

/// Timestamp in milliseconds since epoch
pub type Timestamp = i64;

/// Generic result wrapper for commands
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResult<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T> ApiResult<T> {
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn err(error: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(error),
        }
    }
}
