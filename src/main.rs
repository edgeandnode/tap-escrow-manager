use std::{collections::HashSet, env, fs, time::Duration};

use alloy_primitives::Address;
use anyhow::{anyhow, Context as _};
use config::Config;
use eventuals::{Eventual, EventualExt, Ptr};
use serde::Deserialize;
use toolshed::url::Url;

use crate::{receipts::track_receipts, subgraph::spawn_poller};

mod config;
mod receipts;
mod subgraph;

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

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    let indexers = active_indexers(
        http_client.clone(),
        "https://api.thegraph.com/subgraphs/name/graphprotocol/graph-network-arbitrum".parse()?,
    );

    tracing::warn!("{:#?}", indexers.value().await.unwrap().as_ref());

    loop {
        tokio::time::sleep(Duration::from_secs(20)).await;
    }
}

pub fn active_indexers(
    http_client: reqwest::Client,
    subgraph_endpoint: Url,
) -> Eventual<Ptr<HashSet<Address>>> {
    let client = subgraph::Client::new(http_client, subgraph_endpoint, None);
    let query = r#"
        indexers(
            block: $block
            orderBy: id
            orderDirection: asc
            first: $first
            where: {
                id_gt: $last
                allocatedTokens_gt: 0
            }
        ) {
            id
        }
    "#;
    #[derive(Clone, Deserialize)]
    struct Indexer {
        id: Address,
    }
    spawn_poller::<Indexer>(client, query.to_string())
        .map(|v| async move { Ptr::new(v.iter().map(|i| i.id).collect()) })
}
