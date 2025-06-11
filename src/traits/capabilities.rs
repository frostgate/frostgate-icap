/// Types of proof systems supported by a chain
#[derive(Debug, Clone, PartialEq)]
pub enum ProofType {
    None,
    ZkSnark,
    ZkStark,
    Groth16,
    Plonk,
    Custom(String),
}

/// Types of finality mechanisms
#[derive(Debug, Clone, PartialEq)]
pub enum FinalityType {
    Probabilistic { confirmations: u32 },
    Deterministic,
    Instant,
}

/// Chain capabilities and features
#[derive(Debug, Clone)]
pub struct ChainCapabilities {
    pub supports_smart_contracts: bool,
    pub supports_native_tokens: bool,
    pub supports_onchain_verification: bool,
    pub max_message_size: usize,
    pub proof_type: ProofType,
    pub finality_type: FinalityType,
    pub max_proof_size: Option<usize>,
    pub supports_parallel_execution: bool,
}

/// Provides information about chain capabilities
pub trait ChainCapabilityProvider {
    /// Get the capabilities of this chain
    fn capabilities(&self) -> ChainCapabilities;
    
    /// Check if a specific feature is supported
    fn supports_feature(&self, feature: &str) -> bool;
} 