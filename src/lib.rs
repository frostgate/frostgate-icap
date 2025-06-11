//! Frostgate ICAP (Interoperable Chain Abstraction Protocol)
//! 
//! This crate provides a unified interface for interacting with multiple blockchain
//! networks through a common abstraction layer.

#![allow(unused_imports)]
#![allow(dead_code)]

pub mod adapters;
pub mod registry;
pub mod traits;
pub mod types;

pub use adapters::{
    EvmAdapter, EvmConfig, EvmContracts,
    SolanaAdapter, SolanaConfig,
    SubstrateAdapter, SubstrateConfig,
    SuiAdapter, SuiConfig,
};
pub use registry::{AdapterRegistry, RegistryConfig};
pub use traits::{
    ChainAdapter, FinalityProvider, MessageProver,
    MessageSubmitter, EventListener, CapabilityProvider,
    EventSubscription,
};
pub use types::{
    AdapterError, ChainCapabilities, ConnectionStatus,
    FinalizedBlock, HealthMetrics, SubmissionOptions,
    TransactionDetails, TransactionStatus, FinalityType,
    ParsedTransaction,
};

pub type Result<T> = std::result::Result<T, IcapError>; 