use std::{collections::BTreeMap, fmt, ops::Deref, path::PathBuf, str::FromStr};

use serde::Deserialize;
use serde_with::{serde_as, DisplayFromStr};
use thegraph::types::{Address, B256};
use toolshed::url::Url;

#[serde_as]
#[derive(Debug, Deserialize)]
pub struct Config {
    pub chain_id: u64,
    pub escrow_contract: Address,
    #[serde_as(as = "DisplayFromStr")]
    pub escrow_subgraph: Url,
    pub graph_env: String,
    pub kafka: Kafka,
    #[serde_as(as = "DisplayFromStr")]
    pub network_subgraph: Url,
    #[serde_as(as = "DisplayFromStr")]
    pub rpc_url: Hidden<Url>,
    pub secret_key: Hidden<B256>,
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
