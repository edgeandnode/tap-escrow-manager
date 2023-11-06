use std::collections::{HashMap, HashSet};
use std::{env, fs, time::Duration};

use alloy_primitives::Address;
use anyhow::{anyhow, Context as _};
use config::Config;
use ethers::middleware::contract::abigen;
use ethers::prelude::{Http, Provider};
use ethers::signers::{LocalWallet, Signer as _};
use eventuals::{Eventual, EventualExt, Ptr};
use serde::Deserialize;
use toolshed::url::Url;

use crate::{receipts::track_receipts, subgraph::spawn_poller};

mod config;
mod receipts;
mod subgraph;

#[global_allocator]
static ALLOC: snmalloc_rs::SnMalloc = snmalloc_rs::SnMalloc;

abigen!(Escrow, "src/abi/Escrow.abi.json");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config_file = env::args()
        .nth(1)
        .ok_or_else(|| anyhow!("missing config file argument"))?;
    let config: Config = fs::read_to_string(config_file)
        .map_err(anyhow::Error::from)
        .and_then(|s| serde_json::from_str(&s).map_err(anyhow::Error::from))
        .context("failed to load config")?;
    tracing::info!("{config:#?}");

    let provider = Provider::<Http>::try_from(config.provider.as_str())?;
    let wallet =
        LocalWallet::from_bytes(config.secret_key.as_slice())?.with_chain_id(config.chain_id);
    tracing::info!(sender_address = %wallet.address());

    let debts = track_receipts(&config.kafka)
        .await
        .context("failed to start kafka client")?;

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    let active_indexers = active_indexers(
        http_client.clone(),
        "https://api.thegraph.com/subgraphs/name/graphprotocol/graph-network-arbitrum".parse()?,
    );
    let escrow_accounts = escrow_accounts(
        http_client,
        "https://api.studio.thegraph.com/proxy/53925/arb-goerli-tap-subgraph/version/latest"
            .parse()?,
        "0x21fed3c4340f67dbf2b78c670ebd1940668ca03e".parse()?,
    );

    tracing::warn!(active_indexers = active_indexers.value().await.unwrap().len());
    tracing::warn!(escrow_accounts = escrow_accounts.value().await.unwrap().len());

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
    #[derive(Deserialize)]
    struct Indexer {
        id: Address,
    }
    spawn_poller::<Indexer>(client, query.to_string())
        .map(|v| async move { Ptr::new(v.iter().map(|i| i.id).collect()) })
}

pub fn escrow_accounts(
    http_client: reqwest::Client,
    subgraph_endpoint: Url,
    sender: Address,
) -> Eventual<Ptr<HashMap<Address, u128>>> {
    let client = subgraph::Client::new(http_client, subgraph_endpoint, None);
    let query = format!(
        r#"
        escrowAccounts(
            block: $block
            orderBy: id
            orderDirection: asc
            first: $first
            where: {{
                id_gt: $last
                sender: "{sender:?}"
            }}
        ) {{
            id
            balance
            receiver {{
                id
            }}
        }}
        "#
    );
    #[derive(Deserialize)]
    struct EscrowAccount {
        balance: String,
        receiver: Receiver,
    }
    #[derive(Deserialize)]
    struct Receiver {
        id: Address,
    }
    spawn_poller::<EscrowAccount>(client, query.to_string()).map(|v| async move {
        let entries = v
            .iter()
            .map(|account| {
                let balance = account.balance.parse().expect("failed to parse balance");
                (account.receiver.id, balance)
            })
            .collect();
        Ptr::new(entries)
    })
}
