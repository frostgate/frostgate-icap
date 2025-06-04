# Substrate Chain Adapter

The Substrate Chain Adapter is a production-grade implementation of the Frostgate Chain Adapter interface for Polkadot/Substrate-based chains. It provides a robust, performant, and type-safe way to interact with any Substrate-based blockchain.

## Features

- **Full Chain Adapter Implementation**: Implements all required methods from the `ChainAdapter` trait
- **Runtime Version Management**: Automatic handling of runtime upgrades and metadata updates
- **Transaction Management**: Comprehensive transaction tracking and status monitoring
- **Event Handling**: Efficient event filtering and processing
- **Health Monitoring**: Extensive metrics and health checks
- **Resource Management**: Configurable concurrent request limits and rate limiting
- **Error Handling**: Robust error handling with detailed error types
- **Type Safety**: Full type safety through Substrate's metadata system

## Configuration

### Basic Configuration

```rust
use frostgate_icap::substrate::types::PolkadotConfig;

let config = PolkadotConfig::mainnet(
    "wss://rpc.polkadot.io".to_string(),
    "your-seed-phrase".to_string(),
);
```

### Configuration Options

- `rpc_url`: WebSocket or HTTP RPC endpoint URL
- `signer_seed`: Seed phrase or private key for transaction signing
- `confirmations`: Number of block confirmations required (default: 12)
- `rpc_timeout`: Timeout for RPC requests (default: 30s)
- `max_concurrent_requests`: Maximum concurrent RPC requests (default: 10)
- `health_check_interval`: Interval between health checks (default: 30s)
- `min_call_interval`: Minimum interval between RPC calls (default: 100ms)
- `message_pallet`: Custom pallet name for cross-chain messaging (default: "XcmpQueue")
- `chain_specific`: Chain-specific configuration overrides

### Preset Configurations

```rust
// Polkadot Mainnet
let mainnet = PolkadotConfig::mainnet(rpc_url, signer_seed);

// Kusama Network
let kusama = PolkadotConfig::kusama(rpc_url, signer_seed);

// Development/Testing
let dev = PolkadotConfig::development(rpc_url, signer_seed);
```

## Usage

### Initialization

```rust
use frostgate_icap::substrate::adapter::PolkadotAdapter;

let adapter = PolkadotAdapter::new(config).await?;
```

### Message Submission

```rust
let message = FrostMessage::new(
    ChainId::Polkadot,
    ChainId::Ethereum,
    payload,
    nonce,
    timestamp,
);

let tx_hash = adapter.submit_message(&message).await?;
```

### Transaction Monitoring

```rust
// Check message status
let status = adapter.message_status(&message_id).await?;

// Wait for finality
adapter.wait_for_finality(&block_number).await?;
```

### Event Listening

```rust
// Get new events
let events = adapter.listen_for_events().await?;

// Process events
for event in events {
    adapter.handle_event(event).await?;
}
```

## Metrics and Monitoring

### Available Metrics

- API call statistics (success/failure rates, latencies)
- Resource usage (memory, file descriptors, threads)
- Connection health status
- Block progression
- Transaction states

### Accessing Metrics

```rust
// Get all metrics
let metrics = adapter.get_metrics().await;

// Get specific operation stats
let submit_stats = adapter.get_operation_stats("submit_message").await;

// Get error rate for operation
let error_rate = adapter.get_error_rate("submit_message").await;

// Get average latency
let latency = adapter.get_average_latency("submit_message").await;
```

## Error Handling

The adapter provides detailed error types through `AdapterError`:

- `Network`: Connection and RPC-related errors
- `Configuration`: Setup and configuration errors
- `InvalidMessage`: Message format or content errors
- `ProofError`: Proof verification failures
- `Timeout`: Operation timeout errors
- `Serialization`: Data encoding/decoding errors

## Constants

```rust
const DEFAULT_CONFIRMATIONS: u32 = 12;
const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RETRY_ATTEMPTS: u32 = 3;
const SUBSTRATE_BLOCK_TIME: Duration = Duration::from_secs(6);
const MAX_FINALITY_WAIT_TIME: Duration = Duration::from_secs(600);
```

## Internal Types

### TransactionInfo

Tracks the state of submitted transactions:
```rust
pub struct TransactionInfo {
    pub message_id: Uuid,
    pub tx_hash: H256,
    pub block_number: Option<u32>,
    pub status: TxTrackingStatus,
    pub submitted_at: Instant,
    pub confirmations: u32,
    pub error: Option<String>,
}
```

### ApiCallStats

Tracks API call performance:
```rust
pub struct ApiCallStats {
    pub total_calls: u64,
    pub successful_calls: u64,
    pub failed_calls: u64,
    pub total_latency: Duration,
    pub average_latency: Duration,
    pub max_latency: Duration,
    pub last_error: Option<String>,
}
```

## Best Practices

1. **Runtime Updates**: Always check for runtime upgrades before submitting transactions
2. **Error Handling**: Implement proper error handling and retries for network operations
3. **Resource Management**: Configure appropriate request limits based on chain capacity
4. **Monitoring**: Regularly check health metrics and implement alerting
5. **Event Processing**: Process events in order and handle missed events
6. **Transaction Tracking**: Monitor transaction status and implement proper finality checks

## Dependencies

- `subxt`: Substrate API client
- `sp-keyring`: Substrate key management
- `async-trait`: Async trait support
- `tokio`: Async runtime
- `tracing`: Logging and instrumentation

## Contributing

When contributing to the Substrate adapter:

1. Ensure all changes maintain type safety
2. Add appropriate error handling
3. Update metrics for new operations
4. Add tests for new functionality
5. Document changes in code and README
6. Follow existing code style and patterns

## Testing

The adapter includes comprehensive tests:

```rust
#[tokio::test]
async fn test_submit_message() {
    // Test implementation
}
```

Run tests with:
```bash
cargo test --package frostgate-icap --lib -- substrate::tests
```
