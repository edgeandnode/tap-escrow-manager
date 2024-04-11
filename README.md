# tap-escrow-manager

This service maintains TAP escrow balances on behalf of a gateway sender.

The following data sources are monitored to guide the allocation of GRT into the [TAP Escrow contract](https://github.com/semiotic-ai/timeline-aggregation-protocol-contracts):

- [Graph Network Subgraph](https://github.com/graphprotocol/graph-network-subgraph)
- [TAP Subgraph](https://github.com/semiotic-ai/timeline-aggregation-protocol-subgraph)
- The gateway's Kafka topic for indexer query reports, to account for outstanding debts from receipts sent to indexers
  - Note: Kafka messages are processed for the past 28 days (Escrow `withdrawEscrowThawingPeriod`). Daily aggregations are stored to a file to avoid reprocessing on restart.

# Configuration

Configuration options are set via a single JSON file. The structure of the file is defined in [src/config.rs](src/config.rs).

# Logs

Log levels are controlled by the `RUST_LOG` environment variable ([details](https://docs.rs/env_logger/latest/env_logger/#enabling-logging)).

example: `RUST_LOG=info,tap_escrow_manager=debug cargo run -- config.json`
