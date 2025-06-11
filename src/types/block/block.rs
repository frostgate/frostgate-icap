use std::time::SystemTime;

/// A block that has reached finality
#[derive(Debug, Clone)]
pub struct FinalizedBlockId<T> {
    /// The underlying block identifier
    pub block: T,
    /// Optional proof of finality
    pub finality_proof: Option<Vec<u8>>,
    /// When the block was finalized
    pub finalized_at: SystemTime,
    /// Number of confirmations (if applicable)
    pub confirmations: Option<u32>,
}

impl<T> FinalizedBlockId<T> {
    /// Create a new finalized block ID
    pub fn new(
        block: T,
        finality_proof: Option<Vec<u8>>,
        confirmations: Option<u32>,
    ) -> Self {
        Self {
            block,
            finality_proof,
            finalized_at: SystemTime::now(),
            confirmations,
        }
    }

    /// Get the underlying block ID
    pub fn into_inner(self) -> T {
        self.block
    }
} 