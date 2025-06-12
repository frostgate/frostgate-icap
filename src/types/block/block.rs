use std::time::SystemTime;
pub use frostgate_sdk::types::FinalizedBlock;

/// Extension trait for FinalizedBlock functionality
pub trait FinalizedBlockExt<T> {
    /// Get the underlying block ID
    fn into_inner(self) -> T;
}

impl<T> FinalizedBlockExt<T> for FinalizedBlock<T> {
    fn into_inner(self) -> T {
        self.block
    }
} 