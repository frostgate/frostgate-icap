use async_trait::async_trait;
use uuid::Uuid;
use crate::types::transaction::{TransactionDetails, ProofData};
use crate::types::error::{ProofError, SubmissionError, FeeEstimationError};

/// Trait for messages that can be sent across chains
pub trait CrossChainMessage: Send + Sync {
    fn id(&self) -> Uuid;
    fn payload(&self) -> &[u8];
    fn chain_specific_data(&self) -> Option<&[u8]>;
}

/// Handles proof generation and verification
#[async_trait]
pub trait MessageProver: Send + Sync {
    /// Generate a proof for a message
    async fn generate_proof(&self, message: &impl CrossChainMessage) -> Result<ProofData, ProofError>;
    
    /// Verify a proof
    async fn verify_proof(&self, proof: &ProofData) -> Result<bool, ProofError>;
}

/// Handles message submission to chains
#[async_trait]
pub trait MessageSubmitter: Send + Sync {
    /// The type used to identify transactions in this chain
    type TxId: Clone + std::fmt::Debug + Send + Sync + 'static;
    
    /// Submit a message to the chain
    async fn submit_message<M: CrossChainMessage>(&self, msg: &M) -> Result<Self::TxId, SubmissionError>;
    
    /// Estimate the fee for submitting a message
    async fn estimate_fee<M: CrossChainMessage>(&self, msg: &M) -> Result<u128, FeeEstimationError>;
    
    /// Get transaction details
    async fn get_transaction(&self, tx: &Self::TxId) -> Result<Option<TransactionDetails>, SubmissionError>;
} 