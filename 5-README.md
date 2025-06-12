# Frostgate ICAP (Interoperable Chain Abstraction Protocol)

A robust and flexible protocol implementation that provides unified interfaces for interacting with multiple blockchain networks including Ethereum, Solana, and Polkadot/Substrate chains. ICAP modular design allows for intergrating new chains by implementing ChainAdapter trait for it.

## Features

- **Multi-Chain Support**
  - Ethereum (via ethers)
  - Solana (via solana-client)
  - Polkadot/Substrate (via subxt)

- **Unified Interface**
  - Common transaction handling
  - Standardized error types
  - Consistent API patterns across chains

- **Advanced Capabilities**
  - Zero-Knowledge Integration (via frostgate-zkip)
  - Proof Generation (via frostgate-prover)
  - Verification Systems (via frostgate-verifier)

- **Robust Error Handling**
  - Chain-specific error mapping
  - Comprehensive error types
  - Detailed error context

- **Performance Optimized**
  - Rate limiting
  - Retry mechanisms
  - Parallel processing support

## Dependencies

- Rust 2024 Edition
- Core Dependencies:
  - frostgate-sdk
  - frostgate-prover
  - frostgate-verifier
  - frostgate-zkip
- Blockchain SDKs:
  - ethers
  - solana-client (v2.2.2)
  - subxt (v0.42.1)

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
frostgate-icap = { path = "path/to/frostgate-icap" }
```

## Usage

```rust
use frostgate_icap::{
    ChainAdapter,
    EthereumAdapter,
    SolanaAdapter,
    PolkadotAdapter,
};

// Initialize adapters
let eth_adapter = EthereumAdapter::new(config);
let sol_adapter = SolanaAdapter::new(config);
let dot_adapter = PolkadotAdapter::new(config);

// Use unified interface across chains
async fn example() {
    // Transaction handling is consistent across chains
    let tx = adapter.get_transaction(&tx_id).await?;
    
    // Error handling is standardized
    match result {
        Ok(data) => // Process data
        Err(AdapterError::Network(e)) => // Handle network error
        Err(AdapterError::Serialization(e)) => // Handle serialization error
        // ...
    }
}
```

## Configuration

Each chain adapter can be configured with specific parameters:

```rust
// Ethereum configuration
let eth_config = EthereumConfig {
    rpc_url: "https://mainnet.infura.io/v3/YOUR-PROJECT-ID",
    // ...
};

// Solana configuration
let sol_config = SolanaConfig {
    rpc_url: "https://api.mainnet-beta.solana.com",
    commitment: CommitmentLevel::Finalized,
    // ...
};

// Polkadot configuration
let dot_config = PolkadotConfig {
    ws_url: "wss://rpc.polkadot.io",
    // ...
};
```

## Error Handling

The library provides a unified error type `AdapterError` that maps chain-specific errors to a common format:

```rust
pub enum AdapterError {
    Network(String),
    Serialization(String),
    Other(String),
    // ...
}
```

## Contributing

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add some amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## License

This project is licensed under the terms found in [LICENSE](LICENSE).

## Related Projects

- [frostgate-sdk](../frostgate-sdk)
- [frostgate-prover](../frostgate-prover)
- [frostgate-verifier](../frostgate-verifier)
- [frostgate-zkip](../frostgate-zkip) 