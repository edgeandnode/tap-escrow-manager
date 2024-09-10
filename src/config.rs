use std::{collections::BTreeMap, fmt, ops::Deref, path::PathBuf, str::FromStr};

use reqwest::Url;
use serde::Deserialize;
use serde_with::{serde_as, DisplayFromStr};
use thegraph_core::types::alloy_primitives::{Address, B256};

#[serde_as]
#[derive(Debug, Deserialize)]
pub struct Config {
    /// Authorize signers on startup.
    pub authorize_signers: bool,
    /// ID for the chain where the TAP escrow contract is deployed
    pub chain_id: u64,
    /// Table of minimum debts by indexer. This can be used, for example, to account for receipts
    /// missing from the kafka topic.
    pub debts: BTreeMap<Address, u64>,
    /// TAP escrow contract address
    pub escrow_contract: Address,
    /// TAP escrow subgraph
    #[serde_as(as = "DisplayFromStr")]
    pub escrow_subgraph: Url,
    /// GRT contract for updating allowance
    pub grt_contract: Address,
    /// GRT allowance to set on startup
    pub grt_allowance: u64,
    /// Kafka configuration
    pub kafka: Kafka,
    /// Graph network subgraph URL
    #[serde_as(as = "DisplayFromStr")]
    pub network_subgraph: Url,
    /// API key for querying subgraphs
    pub query_auth: Hidden<String>,
    /// RPC for executing transactions
    #[serde_as(as = "DisplayFromStr")]
    pub rpc_url: Hidden<Url>,
    /// Secret key of the TAP sender wallet
    pub secret_key: Hidden<B256>,
    /// Secret keys of the TAP signer wallets to authorize. Also filters the indexer fees messages.
    pub signers: Vec<Hidden<B256>>,
    /// Period of the subgraph polling cycle
    pub update_interval_seconds: u32,
}

#[derive(Debug, Deserialize)]
pub struct Kafka {
    pub config: Hidden<BTreeMap<String, String>>,
    pub cache: PathBuf,
    pub topic: String,
}

#[derive(Deserialize)]
#[serde(transparent)]
pub struct Hidden<T>(T);

impl<T: fmt::Debug> fmt::Debug for Hidden<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HIDDEN")
    }
}

impl<T: FromStr> FromStr for Hidden<T> {
    type Err = T::Err;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse()?))
    }
}

impl<T> Deref for Hidden<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
