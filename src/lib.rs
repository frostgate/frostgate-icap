//! Frostgate ICAP (Interoperable Chain Abstraction Protocol)
//! 
//! This crate provides a unified interface for interacting with multiple blockchain
//! networks through a common abstraction layer.

#![allow(unused_imports)]
#![allow(dead_code)]

pub mod adapter_registry;
pub mod chainadapter;
pub mod error;
pub mod types;

pub mod evm;
pub mod solana;
pub mod substrate;
pub mod sui;

pub use adapter_registry::AdapterRegistry;
pub use chainadapter::{ChainAdapter, AdapterError};
pub use error::IcapError;
pub use types::{ChainConfig, *};

pub type Result<T> = std::result::Result<T, IcapError>; 