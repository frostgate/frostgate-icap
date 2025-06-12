pub mod finality;
pub mod message;
pub mod events;
pub mod capabilities;

pub use finality::*;
pub use message::*;
pub use events::*;
pub use capabilities::*;

use async_trait::async_trait;
use crate::types::{
    AdapterError, ChainCapabilities, ConnectionStatus, FinalizedBlock,
    HealthMetrics, SubmissionOptions, TransactionDetails,
};
use frostgate_sdk::frostmessage::{FrostMessage, MessageEvent};
use std::time::Duration;

// Re-export SDK traits
pub use frostgate_sdk::traits::{
    FinalityProvider,
    MessageProver,
    MessageSubmitter,
    EventListener,
    CapabilityProvider,
    EventSubscription,
    ChainAdapter,
};

/// Provides finality-related functionality for a blockchain
#[async_trait]
pub trait FinalityProvider: Send + Sync {
    /// The type used to identify blocks in this chain
    type BlockId: Clone + std::fmt::Debug + Send + Sync + 'static;

    /// Get the latest finalized block
    async fn latest_finalized_block(&self) -> Result<FinalizedBlock<Self::BlockId>, AdapterError>;

    /// Wait for a block to reach finality
    async fn wait_for_finality(
        &self,
        block: &Self::BlockId,
        timeout: Option<Duration>,
    ) -> Result<FinalizedBlock<Self::BlockId>, AdapterError>;

    /// Check if a block is finalized
    async fn is_finalized(&self, block: &Self::BlockId) -> Result<bool, AdapterError>;
}

/// Handles proof generation and verification
#[async_trait]
pub trait MessageProver: Send + Sync {
    /// Generate a proof for a message
    async fn generate_proof(&self, message: &FrostMessage) -> Result<Vec<u8>, AdapterError>;

    /// Verify a proof
    async fn verify_proof(&self, message: &FrostMessage) -> Result<bool, AdapterError>;
}

/// Handles message submission to chains
#[async_trait]
pub trait MessageSubmitter: Send + Sync {
    /// The type used to identify transactions in this chain
    type TxId: Clone + std::fmt::Debug + Send + Sync + 'static;

    /// Submit a message to the chain
    async fn submit_message(
        &self,
        message: &FrostMessage,
        options: Option<SubmissionOptions>,
    ) -> Result<Self::TxId, AdapterError>;

    /// Get transaction details
    async fn get_transaction(
        &self,
        tx_id: &Self::TxId,
    ) -> Result<Option<TransactionDetails>, AdapterError>;

    /// Estimate fee for submitting a message
    async fn estimate_fee(&self, message: &FrostMessage) -> Result<u128, AdapterError>;
}

/// Handles event listening and filtering
#[async_trait]
pub trait EventListener: Send + Sync {
    /// Listen for new message events
    async fn listen_for_events(&self) -> Result<Vec<MessageEvent>, AdapterError>;

    /// Filter events by criteria
    async fn filter_events(
        &self,
        from_block: Option<u64>,
        to_block: Option<u64>,
        event_types: Option<Vec<String>>,
    ) -> Result<Vec<MessageEvent>, AdapterError>;

    /// Subscribe to new events
    async fn subscribe(&self) -> Result<EventSubscription, AdapterError>;
}

/// Event subscription handle
pub struct EventSubscription {
    /// Function to unsubscribe from events
    pub unsubscribe: Box<dyn Fn() -> Result<(), AdapterError> + Send + Sync>,
}

/// Provides chain capability information
#[async_trait]
pub trait CapabilityProvider: Send + Sync {
    /// Get chain capabilities
    async fn capabilities(&self) -> Result<ChainCapabilities, AdapterError>;

    /// Check if a specific capability is supported
    async fn supports_capability(&self, capability: &str) -> Result<bool, AdapterError>;

    /// Get chain connection status
    async fn connection_status(&self) -> Result<ConnectionStatus, AdapterError>;

    /// Get detailed health metrics
    async fn health_metrics(&self) -> Result<HealthMetrics, AdapterError>;
}

/// Combined trait for full chain adapter functionality
pub trait ChainAdapter:
    FinalityProvider + MessageProver + MessageSubmitter + EventListener + CapabilityProvider
{
    /// Get the chain ID this adapter handles
    fn chain_id(&self) -> frostgate_sdk::frostmessage::ChainId;

    /// Get a unique identifier for this adapter instance
    fn adapter_id(&self) -> String;
} 