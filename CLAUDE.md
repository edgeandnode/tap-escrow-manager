# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

tap-escrow-manager is a Rust service that maintains TAP (Timeline Aggregation Protocol) escrow balances on behalf of a gateway sender. It monitors multiple data sources to guide GRT token allocation into the TAP Escrow contract:

- Graph Network Subgraph
- TAP Subgraph  
- Kafka topic for indexer query reports

## Development Commands

### Build and Test
```bash
# Build the project
cargo build

# Run tests
cargo test

# Run linting (with all warnings as errors)
cargo clippy -- -Dwarnings --force-warn deprecated --force-warn dead-code

# Format code (use nightly for formatting)
cargo +nightly fmt

# Type checking
cargo check
```

### Running the Service
```bash
# Run with config file
cargo run -- config.json

# Run with debug logging
RUST_LOG=info,tap_escrow_manager=debug cargo run -- config.json
```

### Useful Kafka Commands
```bash
# Seek Kafka consumer group to specific timestamp
rpk group seek tap-escrow-manager-mainnet --to $unix_timestamp
```

## Architecture

### Key Components

1. **Main Service Loop** (`src/main.rs`): Orchestrates the service, manages configuration, initializes contracts and signers, and runs the main update loop.

2. **Contracts Module** (`src/contracts.rs`): Handles interactions with Ethereum contracts (GRT token and TAP Escrow) using Alloy.

3. **Kafka Module** (`src/kafka.rs`): Consumes receipt messages from Kafka to track outstanding debts from indexers.

4. **Subgraphs Module** (`src/subgraphs.rs`): Queries Graph Network and TAP subgraphs for active allocations, authorized signers, and escrow account balances.

5. **Configuration** (`src/config.rs`): Defines the JSON configuration structure including RPC URLs, contract addresses, Kafka settings, and signer keys.

### Core Logic Flow

1. Service initializes with authorized signers and sets GRT allowance
2. Monitors Kafka for receipt messages to track debts:
   - Individual receipts from gateway via `gateway_queries` topic
   - Aggregated hourly fees from titorelli via `gateway_indexer_fees_hourly` topic
   - RAVs from tap_aggregator via `gateway_ravs` topic to avoid double-counting
3. Periodically polls subgraphs to:
   - Get active allocations for indexers
   - Check current escrow balances
   - Calculate required adjustments
4. Calculates net debt per indexer:
   - Adds receipts from Kafka topics
   - Subtracts RAV amounts (already aggregated receipts)
   - Applies minimum debt thresholds from config
5. Deposits or withdraws GRT to maintain appropriate escrow balances based on:
   - Active allocations
   - Net outstanding debts
   - Configured minimum debts per indexer

### Important Constants

- `GRT`: 10^18 (1 GRT in wei)
- `MIN_DEPOSIT`: 2 GRT minimum deposit amount
- `MAX_ADJUSTMENT`: 10,000 GRT maximum single adjustment

## Inputs and Outputs

### Inputs

1. **Configuration File** (JSON)
   - Contract addresses (GRT token, TAP Escrow)
   - RPC URL for blockchain interaction
   - Subgraph URLs (Network and Escrow)
   - Kafka configuration and topics
   - Private keys for sender and signers
   - Minimum debt mappings per indexer

2. **Kafka Messages**
   - **realtime_topic** (configured as `"gateway_queries"`): Individual query receipts from gateway
     - Producer: edgeandnode/gateway
     - Format: ClientQueryProtobuf messages
   - **aggregated_topic** (configured as `"gateway_indexer_fees_hourly"`): Hourly aggregated fee data
     - Producer: titorelli (aggregation service)
     - Format: IndexerFeesHourlyProtobuf messages
   - **gateway_ravs topic**: Receipt Aggregate Vouchers (RAVs) from tap_aggregator
     - Producer: semiotic-ai/tap_aggregator
     - Key format: `"signer:receiver"` (addresses)
     - Value: RAV amount as string
     - Used to subtract already-aggregated receipts from debt calculations

3. **Graph Network Subgraph** (`network_subgraph` URL from config)
   - **Query**: Active allocations
   ```graphql
   allocations(
       block: $block
       orderBy: id
       orderDirection: asc
       first: $first
       where: {
           id_gt: $last
           status: Active
       }
   ) {
       id
       indexer { id }
   }
   ```
   - Returns: Allocation IDs and their associated indexer addresses

4. **Graph Network Subgraph** (Horizon Integration - same endpoint as #3)
   - **Query 1**: Authorized signers for payer
   ```graphql
   { payer(id:"<payer_address>") { signers { id } } }
   ```
   - Returns: List of authorized signer addresses
   
   - **Query 2**: Escrow account balances
   ```graphql
   paymentsEscrowAccounts(
       block: $block
       orderBy: id
       orderDirection: asc
       first: $first
       where: {
           id_gt: $last
           payer: "<payer_address>"
       }
   ) {
       id
       balance
       receiver { id }
   }
   ```
   - Returns: Escrow balances per receiver (indexer)

5. **Blockchain State** (via RPC at `rpc_url`)
   - **ERC20 Contract** (`grt_contract` address)
     - Method: `allowance(owner, spender)` - Reads current GRT allowance
   - **Chain ID**: Retrieved via RPC for signer authorization

### Outputs

1. **Blockchain Transactions**
   - **ERC20 Contract** (`grt_contract` address)
     - Method: `approve(spender, amount)` - Sets GRT allowance for PaymentsEscrow contract
   
   - **PaymentsEscrow Contract** (`payments_escrow_contract` address)
     - Method: `multicall(calls[])` - Executes batch deposits using multiple `deposit()` calls
     - Individual `deposit(collector, receiver, amount)` calls within multicall
     - Collector address is always the GraphTallyCollector contract address
   
   - **GraphTallyCollector Contract** (`graph_tally_collector_contract` address)
     - Method: `authorizeSigner(signer, deadline, proof)` - Authorizes signers on startup
     - Used for signer authorization and RAV collection (collection handled by others)

2. **Logs**
   - Service status and operation logs
   - Transaction confirmations
   - Error messages and warnings
   - Controlled by `RUST_LOG` environment variable