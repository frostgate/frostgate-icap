pub use frostgate_sdk::traits::{EventListener, EventSubscription};

/// Event filtering options
#[derive(Debug, Clone)]
pub struct EventFilter {
    pub from_block: Option<u64>,
    pub to_block: Option<u64>,
    pub event_types: Vec<String>,
} 