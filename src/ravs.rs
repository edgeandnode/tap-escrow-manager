use std::collections::BTreeMap;

use alloy::primitives::Address;
use anyhow::Context;
use futures_util::StreamExt;
use rdkafka::{consumer::StreamConsumer, Message};
use titorelli::kafka::assign_partitions;
use tokio::sync::watch;

use crate::config;

pub async fn track_ravs(
    config: &config::Kafka,
    signers: Vec<Address>,
) -> anyhow::Result<watch::Receiver<BTreeMap<Address, u128>>> {
    let (tx, rx) = watch::channel(Default::default());
    let mut consumer_config = rdkafka::ClientConfig::from_iter(config.config.clone());
    let defaults = [
        ("group.id", "tap-escrow-manager-testing"),
        ("enable.auto.commit", "true"),
        ("enable.auto.offset.store", "true"),
    ];
    for (key, value) in defaults {
        if !consumer_config.config_map().contains_key(key) {
            consumer_config.set(key, value);
        }
    }
    let mut consumer: StreamConsumer = consumer_config.create()?;
    assign_partitions(&consumer, &["gateway_ravs"], 0).await?;
    tokio::spawn(async move { process_messages(&mut consumer, tx, signers).await });
    Ok(rx)
}

async fn process_messages(
    consumer: &mut StreamConsumer,
    tx: watch::Sender<BTreeMap<Address, u128>>,
    signers: Vec<Address>,
) {
    consumer
        .stream()
        .for_each_concurrent(16, |msg| async {
            let msg = match msg {
                Ok(msg) => msg,
                Err(recv_error) => {
                    tracing::error!(%recv_error);
                    return;
                }
            };
            let record = match parse_record(msg) {
                Ok(record) => record,
                Err(record_parse_err) => {
                    tracing::error!(%record_parse_err);
                    return;
                }
            };
            if !signers.contains(&record.signer) {
                return;
            }
            tx.send_if_modified(|map| {
                match map.entry(record.receiver) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(record.value);
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry)
                        if *entry.get() < record.value =>
                    {
                        entry.insert(record.value);
                    }
                    _ => return false,
                };
                true
            });
        })
        .await;
}

fn parse_record(msg: rdkafka::message::BorrowedMessage) -> anyhow::Result<Record> {
    let key = String::from_utf8_lossy(msg.key().context("missing key")?);
    let payload = String::from_utf8_lossy(msg.payload().context("missing payload")?);
    let (signer, receiver) = key.split_once(':').context("malformed key")?;
    Ok(Record {
        signer: signer.parse()?,
        receiver: receiver.parse()?,
        value: u128::from_str_radix(&payload, 10)?,
    })
}

struct Record {
    signer: Address,
    receiver: Address,
    value: u128,
}

#[cfg(test)]
mod test {
    use std::time::Duration;

    use crate::config::Kafka;
    use alloy::primitives::address;
    use anyhow::Context;

    #[tokio::test]
    async fn wip() -> anyhow::Result<()> {
        tracing_subscriber::fmt::init();

        let config_file = "../secrets/wip.json";
        #[derive(serde::Deserialize)]
        pub struct Config {
            /// Kafka configuration
            pub kafka: Kafka,
        }
        let config: Config = std::fs::read_to_string(config_file)
            .map_err(anyhow::Error::from)
            .and_then(|s| serde_json::from_str(&s).map_err(anyhow::Error::from))
            .context("failed to load config")?;
        let signers = vec![address!("ff4b7a5efd00ff2ec3518d4f250a27e4c29a2211")];
        let mut ravs = super::track_ravs(&config.kafka, signers).await?;
        loop {
            ravs.changed().await?;
            let ravs = ravs.borrow().clone();
            tracing::warn!("{:#?}", ravs);
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}
