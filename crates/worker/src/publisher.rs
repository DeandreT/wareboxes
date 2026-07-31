use std::time::Duration;

use async_trait::async_trait;
use wareboxes_application::outbox::OutboxEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    Retryable,
    Permanent,
}

#[derive(Debug, thiserror::Error)]
#[error("{code}: {message}")]
pub struct PublishError {
    pub class: FailureClass,
    pub code: &'static str,
    pub message: String,
    pub retry_after: Option<Duration>,
}

impl PublishError {
    pub fn retryable(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            class: FailureClass::Retryable,
            code,
            message: message.into(),
            retry_after: None,
        }
    }

    pub fn permanent(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            class: FailureClass::Permanent,
            code,
            message: message.into(),
            retry_after: None,
        }
    }

    pub fn with_retry_after(mut self, retry_after: Duration) -> Self {
        self.retry_after = Some(retry_after);
        self
    }
}

#[async_trait]
pub trait Publisher: Send + Sync + 'static {
    fn name(&self) -> &'static str;

    async fn publish(&self, event: &OutboxEvent) -> Result<(), PublishError>;
}
