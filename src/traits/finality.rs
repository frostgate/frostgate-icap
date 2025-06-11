use async_trait::async_trait;
use crate::types::block::FinalizedBlockId;
use crate::types::error::FinalityError;

/// Provides finality-related functionality for a blockchain
#[async_trait]
pub trait FinalityProvider: Send + Sync {
    /// The type used to identify blocks in this chain
    type BlockId: Clone + std::fmt::Debug + Send + Sync + 'static;
    
    /// Get the latest finalized block
    async fn latest_finalized_block(&self) -> Result<FinalizedBlockId<Self::BlockId>, FinalityError>;
    
    /// Wait for a block to reach finality
    async fn wait_for_finality(&self, block: &Self::BlockId) -> Result<FinalizedBlockId<Self::BlockId>, FinalityError>;
    
    /// Check if a block is finalized
    async fn is_finalized(&self, block: &Self::BlockId) -> Result<bool, FinalityError>;
} 