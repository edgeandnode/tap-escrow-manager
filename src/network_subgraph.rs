use std::{collections::HashSet, time::Duration};

use alloy_primitives::{Address, BlockHash};
use anyhow::anyhow;
use eventuals::{Eventual, EventualExt, Ptr};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex;
use toolshed::thegraph::BlockPointer;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Indexer {
    pub id: Address,
}

pub fn active_indexers(url: String) -> Eventual<Ptr<HashSet<Address>>> {
    let (writer, reader) = Eventual::new();
    let writer: &'static Mutex<_> = Box::leak(Box::new(Mutex::new(writer)));
    eventuals::timer(Duration::from_secs(120))
        .pipe_async(move |_| {
            let url = url.clone();
            async move {
                let active_indexers = match fetch_active_indexers(url).await {
                    Ok(active_indexers) => active_indexers,
                    Err(fetch_allocations_err) => {
                        tracing::error!(%fetch_allocations_err);
                        return;
                    }
                };
                let active_indexers = active_indexers.into_iter().map(|i| i.id).collect();
                writer.lock().await.write(Ptr::new(active_indexers));
            }
        })
        .forever();
    reader
}

async fn fetch_active_indexers(url: String) -> anyhow::Result<Vec<Indexer>> {
    let client = reqwest::Client::new();
    let mut indexers = Vec::new();
    let batch = 1000;
    let mut cursor: Option<Address> = None;
    let mut block_hash: Option<BlockHash> = None;
    loop {
        let query = format!(
            r#"{{
                _meta {{ block {{ number hash }} }}
                indexers(
                    orderBy: id
                    orderDirection: asc
                    first: {batch}
                    {}
                    where: {{
                        {}
                    }}
                ) {{
                    id
                }}
            }}"#,
            block_hash
                .map(|h| format!("block: {{ hash: \"{h}\" }}"))
                .unwrap_or_default(),
            cursor
                .map(|c| format!("id_gt: \"{c}\""))
                .unwrap_or_default(),
        );
        tracing::trace!(?cursor, ?block_hash, "allocations_batch");
        #[derive(Deserialize)]
        struct Response {
            indexers: Vec<Indexer>,
            _meta: Meta,
        }
        #[derive(Deserialize)]
        struct Meta {
            block: BlockPointer,
        }
        let mut response = client
            .post(&url)
            .json(&json!({"query": query}))
            .send()
            .await?
            .json::<graphql::http::Response<Response>>()
            .await?
            .unpack()
            .map_err(|err| anyhow!(err))?;
        let stop = response.indexers.len() < batch;
        cursor = response.indexers.last().map(|i| i.id);
        block_hash.get_or_insert(response._meta.block.hash);
        indexers.append(&mut response.indexers);
        if stop {
            break;
        }
    }

    Ok(indexers)
}
