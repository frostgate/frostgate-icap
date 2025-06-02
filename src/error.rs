use crate::chainadapter::AdapterError;
use ethers::providers::ProviderError;
use subxt::Error as SubxtError;
use solana_client::{
    client_error::{ClientError, ClientErrorKind},
    rpc_request::RpcRequest,
};
use anyhow::Error as AnyhowError;

impl From<ProviderError> for AdapterError {
    fn from(error: ProviderError) -> Self {
        match error {
            ProviderError::JsonRpcClientError(e) => AdapterError::Network(e.to_string()),
            ProviderError::EnsError(e) => AdapterError::Other(format!("ENS error: {}", e)),
            ProviderError::SerdeJson(e) => AdapterError::Serialization(e.to_string()),
            ProviderError::HexError(e) => AdapterError::Other(format!("Hex error: {}", e)),
            _ => AdapterError::Other(format!("Provider error: {:?}", error)),
        }
    }
}

impl From<SubxtError> for AdapterError {
    fn from(error: SubxtError) -> Self {
        match error {
            SubxtError::Rpc(e) => AdapterError::Network(e.to_string()),
            SubxtError::Codec(e) => AdapterError::Serialization(e.to_string()),
            _ => AdapterError::Other(format!("Substrate error: {:?}", error)),
        }
    }
}

impl From<ClientError> for AdapterError {
    fn from(error: ClientError) -> Self {
        match error.kind() {
            ClientErrorKind::RpcError(e) => AdapterError::Network(e.to_string()),
            ClientErrorKind::Custom(e) => AdapterError::Other(e.to_string()),
            _ => AdapterError::Other(format!("Solana client error: {:?}", error)),
        }
    }
}

// Add conversion from String to AdapterError for convenience
impl From<String> for AdapterError {
    fn from(error: String) -> Self {
        AdapterError::Other(error)
    }
}

// Add conversion from &str to AdapterError for convenience
impl From<&str> for AdapterError {
    fn from(error: &str) -> Self {
        AdapterError::Other(error.to_string())
    }
}