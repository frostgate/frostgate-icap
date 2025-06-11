use async_trait::async_trait;
use crate::types::error::EventError;

/// Event filtering options
#[derive(Debug, Clone)]
pub struct EventFilter {
    pub from_block: Option<u64>,
    pub to_block: Option<u64>,
    pub event_types: Vec<String>,
}

/// Event listener for chain events
#[async_trait]
pub trait EventListener: Send + Sync {
    /// Listen for new events
    async fn listen_for_events(&self) -> Result<Vec<MessageEvent>, EventError>;
    
    /// Filter and retrieve specific events
    async fn filter_events(&self, filter: EventFilter) -> Result<Vec<MessageEvent>, EventError>;
    
    /// Subscribe to new events
    async fn subscribe(&self) -> Result<EventSubscription, EventError>;
}

/// Subscription to chain events
pub struct EventSubscription {
    pub unsubscribe: Box<dyn Fn() -> Result<(), EventError> + Send + Sync>,
} 