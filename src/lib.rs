#![allow(unused_imports)]

pub mod adapter_registry;
pub mod chainadapter;
pub mod evm;
pub mod substrate;
pub mod solana;
pub mod types;
pub mod error;

pub use chainadapter::{ChainAdapter, AdapterError};
pub use evm::EthereumAdapter;
pub use solana::SolanaAdapter;
pub use substrate::PolkadotAdapter;
pub use frostgate_sdk::frostmessage::MessageStatus;

// Re-export error types
pub use error::*;