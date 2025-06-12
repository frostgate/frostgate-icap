pub mod block;
pub mod transaction;
pub mod errors;

// Re-export SDK types
pub use frostgate_sdk::types::{
    FinalizedBlock,
    TransactionDetails,
    ParsedTransaction,
    TransactionStatus,
    ChainCapabilities,
    FinalityType,
    AdapterError,
    HealthMetrics,
    ConnectionStatus,
    SubmissionOptions,
};

// Re-export submodule types
pub use block::*;
pub use transaction::*;
pub use errors::*; 