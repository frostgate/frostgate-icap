use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use frostgate_sdk::frostmessage::ChainId;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ConnectionStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Unknown,
}

impl Default for ConnectionStatus {
    fn default() -> Self {
        ConnectionStatus::Unknown
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthMetrics {
    #[serde(skip)]
    pub last_successful_call: Option<Instant>,
    #[serde(serialize_with = "serialize_opt_timestamp", deserialize_with = "deserialize_opt_timestamp")]
    pub last_successful_timestamp: Option<u64>,
    pub consecutive_failures: u32,
    pub total_calls: u64,
    pub failed_calls: u64,
    #[serde(serialize_with = "serialize_duration", deserialize_with = "deserialize_duration")]
    pub avg_response_time: Duration,
    pub connection_status: ConnectionStatus,
    #[serde(skip)]
    pub last_updated: Option<Instant>,
    #[serde(serialize_with = "serialize_timestamp", deserialize_with = "deserialize_timestamp")]
    pub last_updated_timestamp: u64,
    pub latest_block_seen: Option<u32>,
}

// Serialization helpers
fn serialize_timestamp<S>(timestamp: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    timestamp.serialize(serializer)
}

fn deserialize_timestamp<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    u64::deserialize(deserializer)
}

fn serialize_opt_timestamp<S>(timestamp: &Option<u64>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    timestamp.serialize(serializer)
}

fn deserialize_opt_timestamp<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<u64>::deserialize(deserializer)
}

fn serialize_duration<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    duration.as_millis().serialize(serializer)
}

fn deserialize_duration<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let millis = u128::deserialize(deserializer)?;
    Ok(Duration::from_millis(millis as u64))
}

impl Default for HealthMetrics {
    fn default() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
            
        Self {
            last_successful_call: None,
            last_successful_timestamp: None,
            consecutive_failures: 0,
            total_calls: 0,
            failed_calls: 0,
            avg_response_time: Duration::ZERO,
            connection_status: ConnectionStatus::Unknown,
            last_updated: None,
            last_updated_timestamp: now,
            latest_block_seen: None,
        }
    }
}

/// Configuration for connecting to and interacting with a blockchain network
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainConfig {
    /// Chain identifier
    pub chain_id: ChainId,
    
    /// RPC endpoint URL
    pub rpc_url: String,
    
    /// Optional WebSocket endpoint URL
    pub ws_url: Option<String>,
    
    /// Optional authentication token/API key
    pub auth_token: Option<String>,
    
    /// Chain-specific contract addresses
    pub contracts: HashMap<String, String>,
    
    /// Additional chain-specific configuration options
    pub options: HashMap<String, String>,
}

impl ChainConfig {
    /// Create a new chain configuration
    pub fn new(chain_id: ChainId, rpc_url: String) -> Self {
        Self {
            chain_id,
            rpc_url,
            ws_url: None,
            auth_token: None,
            contracts: HashMap::new(),
            options: HashMap::new(),
        }
    }
    
    /// Add a contract address
    pub fn with_contract(mut self, name: &str, address: &str) -> Self {
        self.contracts.insert(name.to_string(), address.to_string());
        self
    }
    
    /// Add a configuration option
    pub fn with_option(mut self, key: &str, value: &str) -> Self {
        self.options.insert(key.to_string(), value.to_string());
        self
    }
    
    /// Set WebSocket URL
    pub fn with_ws_url(mut self, ws_url: String) -> Self {
        self.ws_url = Some(ws_url);
        self
    }
    
    /// Set authentication token
    pub fn with_auth_token(mut self, token: String) -> Self {
        self.auth_token = Some(token);
        self
    }
}