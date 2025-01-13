mod config;
mod contracts;
mod receipts;

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env, fs,
    time::Duration,
};

use alloy::{primitives::Address, signers::local::PrivateKeySigner, sol};
use anyhow::{anyhow, bail, Context};
use config::Config;
use contracts::Contracts;
use serde::Deserialize;
use serde_with::serde_as;
use thegraph_client_subgraphs::{Client as SubgraphClient, PaginatedQueryError};
use tokio::{
    select,
    time::{interval, MissedTickBehavior},
};

use crate::receipts::track_receipts;

#[global_allocator]
static ALLOC: snmalloc_rs::SnMalloc = snmalloc_rs::SnMalloc;

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    ERC20,
    "src/abi/ERC20.abi.json"
);
sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    Escrow,
    "src/abi/Escrow.abi.json"
);

const GRT: u128 = 1_000_000_000_000_000_000;
const MIN_DEPOSIT: u128 = 2 * GRT;
const MAX_ADJUSTMENT: u128 = 10_000 * GRT;

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

    let sender = PrivateKeySigner::from_bytes(&config.secret_key)?;
    tracing::info!(sender = %sender.address());
    let contracts = Contracts::new(
        sender,
        config.rpc_url.clone(),
        config.grt_contract,
        config.escrow_contract,
    );

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let mut network_subgraph = SubgraphClient::builder(http.clone(), config.network_subgraph)
        .with_auth_token(Some(config.query_auth.clone()))
        .build();
    let mut escrow_subgraph = SubgraphClient::builder(http.clone(), config.escrow_subgraph)
        .with_auth_token(Some(config.query_auth.clone()))
        .build();

    let mut signers: Vec<PrivateKeySigner> = Default::default();
    for signer in config.signers {
        let signer = PrivateKeySigner::from_slice(signer.as_slice()).context("load signer key")?;
        signers.push(signer);
    }
    let signers = signers;

    if config.authorize_signers {
        let authorized_signers = authorized_signers(&mut escrow_subgraph, &contracts.sender())
            .await
            .context("fetch authorized signers")?;
        for signer in &signers {
            let authorized = authorized_signers.contains(&signer.address().0.into());
            tracing::info!(signer = %signer.address(), authorized);
            if authorized {
                continue;
            }
            contracts.authorize_signer(signer).await?;
            tracing::info!(signer = %signer.address(), "authorized");
        }
    }

    let allowance = contracts.allowance().await?;
    let expected_allowance = config.grt_allowance as u128 * GRT;
    tracing::info!(allowance = allowance as f64 * 1e-18);
    if allowance < expected_allowance {
        contracts.approve(expected_allowance).await?;
        let allowance = contracts.allowance().await?;
        tracing::info!(allowance = allowance as f64 * 1e-18);
    }

    let signers = signers.into_iter().map(|s| s.address()).collect();
    let debts = track_receipts(&config.kafka, signers)
        .await
        .context("failed to start kafka client")?;

    let mut interval = interval(Duration::from_secs(config.update_interval_seconds as u64));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    loop {
        select! {
            _ = interval.tick() => (),
            _ = tokio::signal::ctrl_c() => bail!("exit"),
            _ = sigterm.recv() => bail!("exit"),
        };

        let mut receivers = match active_indexers(&mut network_subgraph).await {
            Ok(receivers) => receivers,
            Err(active_indexers_err) => {
                tracing::error!("{:#}", active_indexers_err.context("active indexers"));
                continue;
            }
        };
        let escrow_accounts = match escrow_accounts(&mut escrow_subgraph, &contracts.sender()).await
        {
            Ok(escrow_accounts) => escrow_accounts,
            Err(escrow_accounts_err) => {
                if escrow_accounts_err.to_string().contains("missing block") {
                    tracing::warn!("{:#}", escrow_accounts_err.context("escrow accounts"));
                } else {
                    tracing::error!("{:#}", escrow_accounts_err.context("escrow accounts"));
                }
                continue;
            }
        };
        receivers.extend(escrow_accounts.keys());
        tracing::debug!(receivers = receivers.len());

        let debts = debts.borrow();
        let adjustments: Vec<(Address, u128)> = receivers
            .into_iter()
            .filter_map(|receiver| {
                let balance = escrow_accounts.get(&receiver).cloned().unwrap_or(0);
                let debt = u128::max(
                    debts.get(&receiver).copied().unwrap_or(0),
                    config.debts.get(&receiver).copied().unwrap_or(0) as u128 * GRT,
                );
                let next_balance = next_balance(debt);
                let adjustment = next_balance.saturating_sub(balance);
                if adjustment == 0 {
                    return None;
                }
                tracing::info!(
                    ?receiver,
                    balance_grt = (balance as f64) / (GRT as f64),
                    debt_grt = (debt as f64) / (GRT as f64),
                    adjustment_grt = (adjustment as f64) / (GRT as f64),
                );
                Some((receiver, adjustment))
            })
            .collect();
        drop(debts);

        let total_adjustment: u128 = adjustments.iter().map(|(_, a)| a).sum();
        tracing::info!(total_adjustment_grt = ((total_adjustment as f64) * 1e-18).ceil() as u64);
        if total_adjustment > 0 {
            let adjustments = if total_adjustment <= MAX_ADJUSTMENT {
                adjustments
            } else {
                reduce_adjustments(adjustments)
            };
            let tx_block = match contracts.deposit_many(adjustments).await {
                Ok(block) => block,
                Err(deposit_err) => {
                    tracing::error!("{:#}", deposit_err.context("deposit"));
                    continue;
                }
            };
            escrow_subgraph =
                SubgraphClient::builder(escrow_subgraph.http_client, escrow_subgraph.subgraph_url)
                    .with_auth_token(Some(config.query_auth.clone()))
                    .with_subgraph_latest_block(tx_block)
                    .build();

            tracing::info!("adjustments complete");
        }
    }
}

fn next_balance(debt: u128) -> u128 {
    let mut next_round = (MIN_DEPOSIT / GRT) as u32;
    while (debt as f64) >= ((next_round as u128 * GRT) as f64 * 0.6) {
        next_round = next_round
            .saturating_mul(2)
            .min(next_round + (MAX_ADJUSTMENT / GRT) as u32);
    }
    next_round as u128 * GRT
}

fn reduce_adjustments(adjustments: Vec<(Address, u128)>) -> Vec<(Address, u128)> {
    let desired: BTreeMap<Address, u128> = adjustments.into_iter().collect();
    assert!(desired.values().sum::<u128>() > MAX_ADJUSTMENT);
    let mut adjustments: BTreeMap<Address, u128> =
        desired.keys().map(|r| (*r, MIN_DEPOSIT)).collect();
    loop {
        for (receiver, desired_value) in &desired {
            let adjustment_value = adjustments.entry(*receiver).or_default();
            if *adjustment_value < *desired_value {
                *adjustment_value = (*desired_value).min(*adjustment_value + (100 * GRT));
            }
            if adjustments.values().sum::<u128>() >= MAX_ADJUSTMENT {
                return adjustments.into_iter().collect();
            }
        }
    }
}

async fn authorized_signers(
    escrow_subgraph: &mut SubgraphClient,
    sender: &Address,
) -> anyhow::Result<Vec<Address>> {
    #[derive(Deserialize)]
    struct Data {
        sender: Option<Sender>,
    }
    #[derive(Deserialize)]
    struct Sender {
        signers: Vec<Signer>,
    }
    #[derive(Deserialize)]
    struct Signer {
        id: Address,
    }
    let data = escrow_subgraph
        .query::<Data>(format!(
            r#"{{ sender(id:"{sender:?}") {{ signers {{ id }} }} }}"#,
        ))
        .await
        .map_err(|err| anyhow!(err))?;
    let signers = data
        .sender
        .into_iter()
        .flat_map(|s| s.signers)
        .map(|s| s.id)
        .collect();
    Ok(signers)
}

async fn active_indexers(
    network_subgraph: &mut SubgraphClient,
) -> anyhow::Result<HashSet<Address>> {
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
    Ok(network_subgraph
        .paginated_query::<Indexer>(query, 500)
        .await
        .map_err(|err| anyhow!(err))?
        .into_iter()
        .map(|i| i.id)
        .collect())
}

async fn escrow_accounts(
    escrow_subgraph: &mut SubgraphClient,
    sender: &Address,
) -> anyhow::Result<HashMap<Address, u128>> {
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
    #[serde_as]
    #[derive(Deserialize)]
    struct EscrowAccount {
        #[serde_as(as = "serde_with::DisplayFromStr")]
        balance: u128,
        receiver: Receiver,
    }
    #[derive(Deserialize)]
    struct Receiver {
        id: Address,
    }
    let response = escrow_subgraph
        .paginated_query::<EscrowAccount>(query, 500)
        .await;
    match response {
        Ok(accounts) => Ok(accounts
            .into_iter()
            .map(|a| (a.receiver.id, a.balance))
            .collect()),
        Err(PaginatedQueryError::EmptyResponse) => Ok(Default::default()),
        Err(err) => Err(anyhow!(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::{GRT, MIN_DEPOSIT};

    #[test]
    fn next_balance() {
        let tests = [
            (0, MIN_DEPOSIT),
            (GRT, MIN_DEPOSIT),
            (MIN_DEPOSIT / 2, MIN_DEPOSIT),
            (MIN_DEPOSIT, MIN_DEPOSIT * 2),
            (MIN_DEPOSIT + 1, MIN_DEPOSIT * 2),
            (30 * GRT, 64 * GRT),
            (70 * GRT, 128 * GRT),
            (100 * GRT, 256 * GRT),
        ];
        for (debt, expected) in tests {
            assert_eq!(super::next_balance(debt), expected);
        }
    }
}
