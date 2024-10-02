mod config;
mod receipts;

use std::{
    collections::{HashMap, HashSet},
    env, fs,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use alloy::{
    network::EthereumWallet,
    primitives::{keccak256, Address, Bytes, U256},
    providers::{Provider, ProviderBuilder},
    signers::{local::PrivateKeySigner, SignerSync},
    sol,
    sol_types::SolValue,
};
use anyhow::{anyhow, bail, Context};
use config::Config;
use serde::Deserialize;
use serde_with::serde_as;
use thegraph_core::client::{Client as SubgraphClient, PaginatedQueryError};
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

    let sender = PrivateKeySigner::from_bytes(&config.secret_key)?;
    tracing::info!(sender = %sender.address());

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(EthereumWallet::from(sender.clone()))
        .on_http(config.rpc_url.clone());
    let chain_id = provider.get_chain_id().await.context("get chain ID")?;
    let escrow = Escrow::new(config.escrow_contract, provider.clone());
    let token = ERC20::new(config.grt_contract, provider.clone());

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
        let authorized_signers = authorized_signers(&mut escrow_subgraph, &sender.address())
            .await
            .context("fetch authorized signers")?;
        for signer in &signers {
            let authorized = authorized_signers.contains(&signer.address().0.into());
            tracing::info!(signer = %signer.address(), authorized);
            if authorized {
                continue;
            }
            let deadline_offset_s = 60;
            let deadline = U256::from(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
                    + deadline_offset_s,
            );
            let mut proof_message = [0u8; 84];
            proof_message[0..32].copy_from_slice(&U256::from(chain_id).to_be_bytes::<32>());
            proof_message[32..64].copy_from_slice(&deadline.to_be_bytes::<32>());
            proof_message[64..].copy_from_slice(&sender.address().0.abi_encode_packed());
            let hash = keccak256(proof_message);
            let signature = signer
                .sign_message_sync(hash.as_slice())
                .context("sign authorization proof")?;
            let proof: Bytes = signature.as_bytes().into();
            escrow
                .authorizeSigner(signer.address(), deadline, proof)
                .send()
                .await
                .context("authorize signer send")?
                .watch()
                .await
                .context("authorize signer")?;
            tracing::info!(signer = %signer.address(), "authorized");
        }
    }

    let allowance = token
        .allowance(sender.address(), *escrow.address())
        .call()
        .await
        .context("get allowance")?
        ._0;
    let expected_allowance = U256::from(config.grt_allowance as u128 * GRT);
    tracing::info!(allowance = f64::from(allowance) * 1e-18);
    if allowance < expected_allowance {
        token
            .approve(*escrow.address(), expected_allowance)
            .send()
            .await
            .context("approve send")?
            .watch()
            .await
            .context("approve")?;
    }
    let allowance: u128 = token
        .allowance(sender.address(), *escrow.address())
        .call()
        .await
        .context("get allowance")?
        ._0
        .saturating_to();
    tracing::info!(allowance = allowance as f64 * 1e-18);

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
                tracing::error!(%active_indexers_err);
                continue;
            }
        };
        let escrow_accounts = match escrow_accounts(&mut escrow_subgraph, &sender.address()).await {
            Ok(escrow_accounts) => escrow_accounts,
            Err(escrow_accounts_err) => {
                tracing::error!(%escrow_accounts_err);
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
            let receivers: Vec<Address> = adjustments.iter().map(|(r, _)| *r).collect();
            let amounts: Vec<U256> = adjustments.iter().map(|(_, a)| U256::from(*a)).collect();
            let tx = escrow.depositMany(receivers, amounts);
            let pending = match tx.send().await {
                Ok(pending) => pending,
                Err(contract_call_err) => {
                    tracing::error!(%contract_call_err);
                    continue;
                }
            };
            let receipt = match pending
                .with_timeout(Some(Duration::from_secs(20)))
                .get_receipt()
                .await
            {
                Ok(receipt) => receipt,
                Err(pending_tx_err) => {
                    tracing::error!(%pending_tx_err);
                    continue;
                }
            };
            if let Some(latest_block) = receipt.block_number {
                escrow_subgraph = SubgraphClient::builder(
                    escrow_subgraph.http_client,
                    escrow_subgraph.subgraph_url,
                )
                .with_auth_token(Some(config.query_auth.clone()))
                .with_subgraph_latest_block(latest_block)
                .build();
            }

            tracing::info!("adjustments complete");
        }
    }
}

fn next_balance(debt: u128) -> u128 {
    let mut next_round = (MIN_DEPOSIT / GRT) as u32;
    while (debt as f64) >= ((next_round as u128 * GRT) as f64 * 0.6) {
        next_round = next_round.saturating_mul(2);
    }
    next_round as u128 * GRT
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
