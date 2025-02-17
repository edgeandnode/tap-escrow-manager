use alloy::primitives::Address;
use anyhow::anyhow;
use serde_with::serde_as;
use std::collections::{HashMap, HashSet};
use thegraph_client_subgraphs::{Client as SubgraphClient, PaginatedQueryError};

pub async fn authorized_signers(
    escrow_subgraph: &mut SubgraphClient,
    sender: &Address,
) -> anyhow::Result<Vec<Address>> {
    #[derive(serde::Deserialize)]
    struct Data {
        sender: Option<Sender>,
    }
    #[derive(serde::Deserialize)]
    struct Sender {
        signers: Vec<Signer>,
    }
    #[derive(serde::Deserialize)]
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

pub async fn escrow_accounts(
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
    #[derive(serde::Deserialize)]
    struct EscrowAccount {
        #[serde_as(as = "serde_with::DisplayFromStr")]
        balance: u128,
        receiver: Receiver,
    }
    #[derive(serde::Deserialize)]
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

pub async fn active_indexers(
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
    #[derive(serde::Deserialize)]
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
