use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::{env, fs, time::Duration};

use anyhow::{anyhow, Context as _};
use config::Config;
use ethers::middleware::contract::abigen;
use ethers::prelude::{Http, Provider, SignerMiddleware};
use ethers::signers::{LocalWallet, Signer as _};
use ethers::types::U256;
use eventuals::{Eventual, EventualExt, Ptr};
use serde::Deserialize;
use thegraph::client::Client as SubgraphClient;
use thegraph::types::Address;
use tokio::sync::Mutex;
use toolshed::url::Url;

use crate::receipts::track_receipts;

mod config;
mod receipts;

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

    let wallet =
        LocalWallet::from_bytes(config.secret_key.as_slice())?.with_chain_id(config.chain_id);
    let sender_address = wallet.address();
    tracing::info!(%sender_address);
    let http_client = reqwest::ClientBuilder::new()
        .tcp_nodelay(true)
        .timeout(Duration::from_secs(10))
        .build()?;
    let provider = Provider::new(Http::new_with_client(config.rpc_url.0.clone(), http_client));
    let provider = Arc::new(SignerMiddleware::new(provider, wallet));
    let contract = Escrow::new(
        ethers::abi::Address::from(config.escrow_contract.0 .0),
        provider.clone(),
    );

    let debts = track_receipts(&config.kafka)
        .await
        .context("failed to start kafka client")?;

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    let active_indexers = active_indexers(http_client.clone(), config.network_subgraph);
    let escrow_accounts = escrow_accounts(
        http_client,
        config.escrow_subgraph,
        Address::from(sender_address.0),
    );

    tracing::info!(active_indexers = active_indexers.value().await.unwrap().len());
    tracing::info!(escrow_accounts = escrow_accounts.value().await.unwrap().len());

    loop {
        let grt = 1_000_000_000_000_000_000_u128;
        let min_deposit = 16 * grt;
        let max_deposit = 10_000 * grt;

        let debts = debts.value_immediate().unwrap_or_default();
        let escrow_accounts = escrow_accounts.value().await.unwrap();
        let mut receivers = active_indexers.value().await.unwrap().as_ref().clone();
        receivers.extend(escrow_accounts.keys());
        let adjustments: Vec<(Address, u128)> = receivers
            .into_iter()
            .filter_map(|receiver| {
                let balance = escrow_accounts.get(&receiver).cloned().unwrap_or(0);
                let debt = debts.get(&receiver).cloned().unwrap_or(0);
                if balance == 0 {
                    let mut next_balance = min_deposit;
                    while next_balance <= (debt * 2) {
                        next_balance *= 2;
                    }
                    tracing::info!(
                        ?receiver,
                        balance_grt = (balance as f64) / (grt as f64),
                        debt_grt = (debt as f64) / (grt as f64),
                        adjustment_grt = (next_balance as f64) / (grt as f64),
                    );
                    return Some((receiver, next_balance));
                }
                let next_balance = (balance + 1).next_power_of_two();
                let utilization =
                    (debt as f64 / grt as f64) / (balance as f64 / grt as f64).max(1.0);
                if (utilization < 0.6) || (next_balance > max_deposit) {
                    return None;
                }
                let next_balance = next_balance.max(min_deposit);
                let adjustment = next_balance - balance;
                tracing::info!(
                    ?receiver,
                    balance_grt = (balance as f64) / (grt as f64),
                    debt_grt = (debt as f64) / (grt as f64),
                    adjustment_grt = (adjustment as f64) / (grt as f64),
                );
                Some((receiver, adjustment))
            })
            .collect();
        let total_adjustment: u128 = adjustments.iter().map(|(_, a)| a).sum();
        tracing::info!(total_adjustment_grt = ((total_adjustment as f64) * 1e-18).ceil() as u64);
        if total_adjustment > 0 {
            let receivers: Vec<ethers::abi::Address> = adjustments
                .iter()
                .map(|(r, _)| ethers::abi::Address::from(r.0 .0))
                .collect();
            let amounts: Vec<ethers::types::U256> =
                adjustments.iter().map(|(_, a)| U256::from(*a)).collect();
            let tx = contract.deposit_many(receivers, amounts);
            let result = tx.send().await;
            if let Err(contract_call_err) = result {
                let revert = contract_call_err.decode_contract_revert::<EscrowErrors>();
                tracing::error!(%contract_call_err, ?revert);
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
        }
        tracing::info!("adjustments complete");
        tokio::time::sleep(Duration::from_secs(60 * 10)).await;
    }
}

pub fn active_indexers(
    http_client: reqwest::Client,
    subgraph_endpoint: Url,
) -> Eventual<Ptr<HashSet<Address>>> {
    let client = SubgraphClient::new(http_client, subgraph_endpoint);
    let query = r#"
        indexers(
            block: $block
            orderBy: id
            orderDirection: asc
            first: $first
            where: {
                id_gt: $last
                allocationCount_gt: 0
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
    let client = SubgraphClient::new(http_client, subgraph_endpoint);
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

fn spawn_poller<T>(client: SubgraphClient, query: String) -> Eventual<Ptr<Vec<T>>>
where
    T: for<'de> Deserialize<'de> + Send + 'static,
    Ptr<Vec<T>>: Send,
{
    let (writer, reader) = Eventual::new();
    let state: &'static Mutex<_> = Box::leak(Box::new(Mutex::new((writer, client))));
    eventuals::timer(Duration::from_secs(120))
        .pipe_async(move |_| {
            let query = query.clone();
            async move {
                let mut guard = state.lock().await;
                match guard.1.paginated_query::<T>(query).await {
                    Ok(response) => guard.0.write(Ptr::new(response)),
                    Err(subgraph_poll_err) => {
                        tracing::error!(%subgraph_poll_err, label = %std::any::type_name::<T>());
                    }
                };
            }
        })
        .forever();
    reader
}
