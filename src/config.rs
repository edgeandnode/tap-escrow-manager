use std::collections::BTreeMap;

use alloy::primitives::{Address, B256};
use reqwest::Url;
use serde::Deserialize;
use serde_with::serde_as;

#[serde_as]
#[derive(Deserialize)]
pub struct Config {
    /// Authorize signers on startup.
    pub authorize_signers: bool,
    /// Table of minimum debts by indexer. This can be used, for example, to account for receipts
    /// missing from the kafka topic.
    pub debts: BTreeMap<Address, u64>,
    /// TAP escrow contract address
    pub escrow_contract: Address,
    /// TAP escrow subgraph
    #[serde_as(as = "serde_with::DisplayFromStr")]
    pub escrow_subgraph: Url,
    /// GRT contract for updating allowance
    pub grt_contract: Address,
    /// GRT allowance to set on startup
    pub grt_allowance: u64,
    /// Kafka configuration
    pub kafka: Kafka,
    /// Graph network subgraph URL
    #[serde_as(as = "serde_with::DisplayFromStr")]
    pub network_subgraph: Url,
    /// API key for querying subgraphs
    pub query_auth: String,
    /// RPC for executing transactions
    #[serde_as(as = "serde_with::DisplayFromStr")]
    pub rpc_url: Url,
    /// Secret key of the TAP sender wallet
    pub secret_key: B256,
    /// Secret keys of the TAP signer wallets, used to filter the indexer fees messages.
    pub signers: Vec<B256>,
    /// Period of the subgraph polling cycle
    pub update_interval_seconds: u32,
}

#[derive(Debug, Deserialize)]
pub struct Kafka {
    pub config: BTreeMap<String, String>,
    pub realtime_topic: String,
    pub aggregated_topic: Option<String>,
}
