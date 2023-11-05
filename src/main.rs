use std::{env, fs, time::Duration};

use anyhow::{anyhow, Context as _};
use config::Config;

use crate::receipts::track_receipts;

mod config;
mod network_subgraph;
mod receipts;

#[global_allocator]
static ALLOC: snmalloc_rs::SnMalloc = snmalloc_rs::SnMalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config_file = env::args()
        .nth(1)
        .ok_or_else(|| anyhow!("missing config file argument"))?;
    let config: Config =
        serde_json::from_str(&fs::read_to_string(config_file).context("failed to load config")?)?;
    tracing::info!("{config:#?}");

    let debts = track_receipts(&config.kafka)
        .await
        .context("failed to start kafka client")?;

    let indexers = network_subgraph::active_indexers(
        "https://api.thegraph.com/subgraphs/name/graphprotocol/graph-network-arbitrum".to_string(),
    );

    tracing::warn!("{:#?}", indexers.value().await.unwrap().as_ref());

    loop {
        tokio::time::sleep(Duration::from_secs(20)).await;
    }
}
