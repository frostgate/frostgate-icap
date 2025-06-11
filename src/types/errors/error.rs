use thiserror::Error;

#[derive(Error, Debug)]
pub enum FinalityError {
    #[error("Block not found: {0}")]
    BlockNotFound(String),
    
    #[error("Finality timeout after {0} seconds")]
    Timeout(u64),
    
    #[error("Network error: {0}")]
    Network(String),
    
    #[error("Invalid finality proof: {0}")]
    InvalidProof(String),
    
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Error, Debug)]
pub enum ProofError {
    #[error("Failed to generate proof: {0}")]
    Generation(String),
    
    #[error("Failed to verify proof: {0}")]
    Verification(String),
    
    #[error("Invalid proof format: {0}")]
    InvalidFormat(String),
    
    #[error("Missing verification key")]
    MissingVerificationKey,
    
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Error, Debug)]
pub enum SubmissionError {
    #[error("Transaction failed: {0}")]
    TransactionFailed(String),
    
    #[error("Insufficient funds")]
    InsufficientFunds,
    
    #[error("Network error: {0}")]
    Network(String),
    
    #[error("Invalid message format: {0}")]
    InvalidMessage(String),
    
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Error, Debug)]
pub enum FeeEstimationError {
    #[error("Estimation failed: {0}")]
    EstimationFailed(String),
    
    #[error("Network error: {0}")]
    Network(String),
    
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Error, Debug)]
pub enum EventError {
    #[error("Failed to subscribe: {0}")]
    SubscriptionFailed(String),
    
    #[error("Network error: {0}")]
    Network(String),
    
    #[error("Invalid event data: {0}")]
    InvalidData(String),
    
    #[error(transparent)]
    Other(#[from] anyhow::Error),
} 