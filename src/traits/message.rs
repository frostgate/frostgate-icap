pub use frostgate_sdk::traits::{MessageProver, MessageSubmitter};

use uuid::Uuid;
use crate::types::transaction::{TransactionDetails, ProofData};
use crate::types::error::{ProofError, SubmissionError, FeeEstimationError};

/// Trait for messages that can be sent across chains
pub trait CrossChainMessage: Send + Sync {
    fn id(&self) -> Uuid;
    fn payload(&self) -> &[u8];
    fn chain_specific_data(&self) -> Option<&[u8]>;
} 