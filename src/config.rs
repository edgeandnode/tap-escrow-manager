use std::{fmt, ops::Deref, path::PathBuf, str::FromStr};

use alloy_primitives::B256;
use serde::Deserialize;
use serde_with::{serde_as, DisplayFromStr};
use toolshed::url::Url;

#[serde_as]
#[derive(Debug, Deserialize)]
pub struct Config {
    pub chain_id: u64,
    pub kafka: Kafka,
    #[serde_as(as = "DisplayFromStr")]
    pub provider: Hidden<Url>,
    pub secret_key: Hidden<B256>,
}

#[derive(Debug, Deserialize)]
pub struct Kafka {
    pub checkpoint_file: PathBuf,
    pub bootstrap_servers: Hidden<String>,
    pub ca_location: String,
    pub sasl_username: String,
    pub sasl_password: Hidden<String>,
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
