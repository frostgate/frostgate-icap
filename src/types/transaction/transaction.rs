pub use frostgate_sdk::types::{
    TransactionDetails,
    ParsedTransaction,
    TransactionStatus,
    ProofData,
};

use std::collections::HashMap;
use serde::{Serialize, Deserialize};

/// Detailed transaction information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransactionDetails {
    /// Raw transaction bytes
    Raw(Vec<u8>),
    /// Parsed transaction data
    Parsed(ParsedTransaction),
    /// Zero-knowledge proof data
    Proof(ProofData),
}

/// Common transaction fields across chains
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedTransaction {
    pub hash: Vec<u8>,
    pub from: Option<Vec<u8>>,
    pub to: Option<Vec<u8>>,
    pub value: u128,
    pub data: Vec<u8>,
    pub status: TransactionStatus,
    pub metadata: HashMap<String, String>,
}

/// Transaction execution status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TransactionStatus {
    Pending,
    Confirmed,
    Failed(String),
    Dropped,
}

/// Zero-knowledge proof data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofData {
    pub proof_type: String,
    pub proof: Vec<u8>,
    pub public_inputs: Vec<Vec<u8>>,
    pub verification_key: Option<Vec<u8>>,
} 