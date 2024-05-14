mod config;
mod receipts;

use crate::receipts::track_receipts;
use anyhow::{anyhow, Context as _};
use config::Config;
use ethers::middleware::contract::abigen;
use ethers::prelude::{Http, Provider, SignerMiddleware};
use ethers::signers::{LocalWallet, Signer as _};
use ethers::types::{Bytes, H256, U256};
use ethers::utils::keccak256;
use serde::Deserialize;
use serde_with::serde_as;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs, time::Duration};
use thegraph_core::client::Client as SubgraphClient;
use thegraph_core::types::alloy_primitives::Address;
use thegraph_core::types::alloy_sol_types::SolValue;
use tokio::time::{interval, MissedTickBehavior};

#[global_allocator]
static ALLOC: snmalloc_rs::SnMalloc = snmalloc_rs::SnMalloc;

abigen!(Escrow, "src/abi/Escrow.abi.json");

const GRT: u128 = 1_000_000_000_000_000_000;
const MIN_DEPOSIT: u128 = 16 * GRT;
const MAX_DEPOSIT: u128 = 10_000 * GRT;

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

    let sender =
        LocalWallet::from_bytes(config.secret_key.as_slice())?.with_chain_id(config.chain_id);
    let sender_address: Address = sender.address().0.into();
    tracing::info!(sender = %sender_address);

    let provider = Provider::new(Http::new_with_client(
        config.rpc_url.clone(),
        reqwest_old::ClientBuilder::new()
            .timeout(Duration::from_secs(10))
            .build()?,
    ));
    let provider = Arc::new(SignerMiddleware::new(provider, sender.clone()));
    let contract = Escrow::new(
        ethers::abi::Address::from(config.escrow_contract.0 .0),
        provider.clone(),
    );

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    let mut network_subgraph = SubgraphClient::new(http_client.clone(), config.network_subgraph);
    let mut escrow_subgraph = SubgraphClient::new(http_client.clone(), config.escrow_subgraph);

    let authorized_signers = authorized_signers(&mut escrow_subgraph, &sender_address)
        .await
        .context("fetch authorized signers")?;
    for signer in config.signers {
        let signer = LocalWallet::from_bytes(signer.as_slice())?.with_chain_id(config.chain_id);
        let authorized = authorized_signers.contains(&signer.address().0.into());
        tracing::info!(signer = %signer.address(), %authorized);
        let deadline_offset_s = 60;
        let deadline: U256 = (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + deadline_offset_s)
            .into();
        let mut proof_message = [0u8; 84];
        U256::from(config.chain_id).to_big_endian(&mut proof_message[0..32]);
        deadline.to_big_endian(&mut proof_message[32..64]);
        proof_message[64..].copy_from_slice(&sender.address().0.abi_encode_packed());
        let hash = H256(keccak256(proof_message));
        let signature = signer
            .sign_message(hash)
            .await
            .context("sign authorization proof")?;
        let mut proof = [0u8; 65];
        signature.r.to_big_endian(&mut proof[0..32]);
        signature.s.to_big_endian(&mut proof[32..64]);
        proof[64] = signature.v as u8;
        let proof: Bytes = proof.into();
        let tx = contract.authorize_signer(signer.address(), deadline, proof);
        match tx.send().await {
            Ok(result) => {
                result.await.context("authorize tx provider error")?;
            }
            Err(err) => match err.decode_contract_revert::<EscrowErrors>() {
                // We may encounter this condition if the subgraph is behind.
                Some(EscrowErrors::SignerAlreadyAuthorized { .. }) => (),
                Some(revert) => {
                    return Err(anyhow!("Revert({revert:?})").context("authorize signer"));
                }
                None => {
                    return Err(anyhow!(err).context("authorize signer"));
                }
            },
        };
        tracing::info!(signer = %signer.address(), "authorized");
    }

    let debts = track_receipts(&config.kafka, config.graph_env)
        .await
        .context("failed to start kafka client")?;

    let mut interval = interval(Duration::from_secs(config.update_interval_seconds as u64));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        interval.tick().await;

        let mut receivers = match active_indexers(&mut network_subgraph).await {
            Ok(receivers) => receivers,
            Err(active_indexers_err) => {
                tracing::error!(%active_indexers_err);
                continue;
            }
        };
        let escrow_accounts = match escrow_accounts(&mut escrow_subgraph, &sender_address).await {
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
                let debt = debts.get(&receiver).cloned().unwrap_or(0);
                let next_balance = next_balance(debt);
                let adjustment = next_balance.saturating_sub(balance);
                if adjustment == 0 {
                    return None;
                }
                tracing::info!(
                    ?receiver,
                    balance_grt = (balance as f64) / (GRT as f64),
                    debt_grt = (debt as f64) / (GRT as f64),
                    adjustment_grt = (next_balance as f64) / (GRT as f64),
                );
                Some((receiver, adjustment))
            })
            .collect();
        drop(debts);

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
            let pending = match tx.send().await {
                Ok(pending) => pending,
                Err(contract_call_err) => {
                    let revert = contract_call_err.decode_contract_revert::<EscrowErrors>();
                    tracing::error!(%contract_call_err, ?revert);
                    continue;
                }
            };
            let completion = match pending.await {
                Ok(completion) => completion,
                Err(pending_tx_err) => {
                    tracing::error!(%pending_tx_err);
                    continue;
                }
            };
            if let Some(latest_block) = completion.and_then(|c| c.block_number) {
                escrow_subgraph = SubgraphClient::builder(
                    escrow_subgraph.http_client,
                    escrow_subgraph.subgraph_url,
                )
                .with_subgraph_latest_block(latest_block.as_u64())
                .build();
            }

            tracing::info!("adjustments complete");
        }
    }
}

fn next_balance(debt: u128) -> u128 {
    let mut next_round = (MIN_DEPOSIT / GRT) as u32;
    if debt >= MAX_DEPOSIT {
        return MAX_DEPOSIT;
    }
    while (debt as f64) >= ((next_round as u128 * GRT) as f64 * 0.6) {
        next_round = next_round.saturating_mul(2);
    }
    (next_round as u128 * GRT).min(MAX_DEPOSIT)
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
        .paginated_query::<Indexer>(query, 200)
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
        .paginated_query::<EscrowAccount>(query, 200)
        .await;
    match response {
        Ok(accounts) => Ok(accounts
            .into_iter()
            .map(|a| (a.receiver.id, a.balance))
            .collect()),
        Err(err) if err == "empty response" => Ok(Default::default()),
        Err(err) => Err(anyhow!(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::{GRT, MAX_DEPOSIT, MIN_DEPOSIT};

    #[test]
    fn next_balance() {
        let tests = [
            (0, MIN_DEPOSIT),
            (3 * GRT, MIN_DEPOSIT),
            (MIN_DEPOSIT / 2, MIN_DEPOSIT),
            (MIN_DEPOSIT, MIN_DEPOSIT * 2),
            (MIN_DEPOSIT + 1, MIN_DEPOSIT * 2),
            (MIN_DEPOSIT + GRT, MIN_DEPOSIT * 2),
            (30 * GRT, 64 * GRT),
            (70 * GRT, 128 * GRT),
            (100 * GRT, 256 * GRT),
            (MAX_DEPOSIT, MAX_DEPOSIT),
            (MAX_DEPOSIT + GRT, MAX_DEPOSIT),
            (1_000_000 * GRT, MAX_DEPOSIT),
            (u128::MAX, MAX_DEPOSIT),
            (MAX_DEPOSIT - 1, MAX_DEPOSIT),
            (MAX_DEPOSIT - GRT, MAX_DEPOSIT),
        ];
        for (debt, expected) in tests {
            assert_eq!(super::next_balance(debt), expected);
        }
    }
}
