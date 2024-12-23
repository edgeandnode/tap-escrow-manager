use std::collections::BTreeMap;

use alloy::{hex::ToHexExt as _, primitives::Address};
use anyhow::{anyhow, Context};
use chrono::{DateTime, Duration, Utc};
use futures_util::StreamExt as _;
use prost::Message as _;
use rdkafka::{
    consumer::{Consumer, StreamConsumer},
    Message,
};
use titorelli::kafka::{assign_partitions, latest_messages};
use tokio::sync::{mpsc, watch};

use crate::config;

pub async fn track_receipts(
    config: &config::Kafka,
    signers: Vec<Address>,
) -> anyhow::Result<watch::Receiver<BTreeMap<Address, u128>>> {
    let window = Duration::days(28);
    let (tx, rx) = watch::channel(Default::default());
    let db = DB::spawn(window, tx);

    let mut consumer_config = rdkafka::ClientConfig::from_iter(config.config.clone());
    let defaults = [
        ("group.id", "tap-escrow-manager"),
        ("enable.auto.commit", "true"),
        ("enable.auto.offset.store", "true"),
    ];
    for (key, value) in defaults {
        if !consumer_config.config_map().contains_key(key) {
            consumer_config.set(key, value);
        }
    }
    let mut consumer: StreamConsumer = consumer_config.create()?;

    let start_timestamp = hourly_timestamp(Utc::now() - window);
    if let Some(aggregated_topic) = &config.aggregated_topic {
        let latest_aggregated_messages = latest_messages(&consumer, &[aggregated_topic]).await?;
        let mut latest_aggregated_offsets: BTreeMap<String, i64> = latest_aggregated_messages
            .into_iter()
            .map(|msg| (format!("{}/{}", msg.topic(), msg.partition()), msg.offset()))
            .collect();
        assign_partitions(&consumer, &[aggregated_topic], start_timestamp).await?;
        let mut latest_aggregated_timestamp = 0;
        let mut stream = consumer.stream();
        while let Some(msg) = stream.next().await {
            let msg = msg?;
            let partition = format!("{}/{}", msg.topic(), msg.partition());
            let offset = msg.offset();
            let payload = msg
                .payload()
                .with_context(|| anyhow!("missing payload at {partition} {offset}"))?;
            let msg = IndexerFeesHourlyProtobuf::decode(payload)?;
            latest_aggregated_timestamp = latest_aggregated_timestamp.max(msg.timestamp);
            for aggregation in &msg.aggregations {
                if !signers.contains(&Address::from_slice(&aggregation.signer)) {
                    continue;
                }
                let update = Update {
                    timestamp: DateTime::from_timestamp_millis(msg.timestamp)
                        .context("timestamp out of range")?,
                    indexer: Address::from_slice(&aggregation.receiver),
                    fee: (aggregation.fee_grt * 1e18) as u128,
                };
                db.send(update).await.unwrap();
            }

            if latest_aggregated_offsets.get(&partition).unwrap() == &offset {
                latest_aggregated_offsets.remove(&partition);
                if latest_aggregated_offsets.is_empty() {
                    break;
                }
            }
        }
        consumer.unassign()?;
        let realtime_start = latest_aggregated_timestamp + Duration::hours(1).num_milliseconds();
        assign_partitions(&consumer, &[&config.realtime_topic], realtime_start).await?;
    } else {
        assign_partitions(&consumer, &[&config.realtime_topic], start_timestamp).await?;
    }
    tokio::spawn(async move {
        if let Err(kafka_consumer_err) = process_messages(&mut consumer, db, signers).await {
            tracing::error!(%kafka_consumer_err);
        }
    });

    Ok(rx)
}

#[derive(prost::Message)]
struct IndexerFeesProtobuf {
    /// 20 bytes (address)
    #[prost(bytes, tag = "1")]
    signer: Vec<u8>,
    /// 20 bytes (address)
    #[prost(bytes, tag = "2")]
    receiver: Vec<u8>,
    #[prost(double, tag = "3")]
    fee_grt: f64,
}

#[derive(prost::Message)]
struct IndexerFeesHourlyProtobuf {
    /// start timestamp for aggregation, in unix milliseconds
    #[prost(int64, tag = "1")]
    timestamp: i64,
    #[prost(message, repeated, tag = "2")]
    aggregations: Vec<IndexerFeesProtobuf>,
}

#[derive(prost::Message)]
pub struct ClientQueryProtobuf {
    // 20 bytes (address)
    #[prost(bytes, tag = "2")]
    pub receipt_signer: Vec<u8>,
    #[prost(message, repeated, tag = "10")]
    pub indexer_queries: Vec<IndexerQueryProtobuf>,
}
#[derive(prost::Message)]
pub struct IndexerQueryProtobuf {
    /// 20 bytes (address)
    #[prost(bytes, tag = "1")]
    pub indexer: Vec<u8>,
    #[prost(double, tag = "6")]
    pub fee_grt: f64,
    #[prost(bool, optional, tag = "12")]
    pub legacy_scalar: Option<bool>,
}

async fn process_messages(
    consumer: &mut StreamConsumer,
    db: mpsc::Sender<Update>,
    signers: Vec<Address>,
) -> anyhow::Result<()> {
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
            let payload = match msg.payload() {
                Some(payload) => payload,
                None => return,
            };
            let timestamp = msg
                .timestamp()
                .to_millis()
                .and_then(|t| DateTime::from_timestamp(t / 1_000, (t % 1_000) as u32 * 1_000))
                .unwrap_or_else(Utc::now);
            let payload = match ClientQueryProtobuf::decode(payload) {
                Ok(payload) => payload,
                Err(payload_parse_err) => {
                    tracing::error!(%payload_parse_err, input = payload.encode_hex());
                    return;
                }
            };
            if !signers.contains(&Address::from_slice(&payload.receipt_signer)) {
                return;
            }
            for indexer_query in payload.indexer_queries {
                if indexer_query.legacy_scalar.unwrap_or(false) {
                    continue;
                }
                let update = Update {
                    timestamp,
                    indexer: Address::from_slice(&indexer_query.indexer),
                    fee: (indexer_query.fee_grt * 1e18) as u128,
                };
                let _ = db.send(update).await;
            }
        })
        .await;
    Ok(())
}

struct Update {
    timestamp: DateTime<Utc>,
    indexer: Address,
    fee: u128,
}

struct DB {
    // indexer debts, aggregated per hour
    data: BTreeMap<Address, BTreeMap<i64, u128>>,
    window: Duration,
    tx: watch::Sender<BTreeMap<Address, u128>>,
}

impl DB {
    pub fn spawn(
        window: Duration,
        tx: watch::Sender<BTreeMap<Address, u128>>,
    ) -> mpsc::Sender<Update> {
        let mut db = Self {
            data: Default::default(),
            window,
            tx,
        };
        let (tx, mut rx) = mpsc::channel(128);
        tokio::spawn(async move {
            let mut last_snapshot = Utc::now();
            let mut last_log = last_snapshot;
            let mut message_count: usize = 0;
            let buffer_size = 128;
            let mut buffer: Vec<Update> = Vec::with_capacity(buffer_size);
            loop {
                rx.recv_many(&mut buffer, buffer_size).await;
                let now = Utc::now();
                message_count += buffer.len();
                for update in buffer.drain(..) {
                    db.update(update, now);
                }

                if (now - last_snapshot) >= Duration::seconds(1) {
                    db.prune(now);
                    let snapshot = db.snapshot();

                    if (now - last_log) >= Duration::minutes(1) {
                        let update_hz = message_count / (now - last_log).num_seconds() as usize;
                        let debts: BTreeMap<&Address, f64> = snapshot
                            .iter()
                            .map(|(k, v)| (k, *v as f64 * 1e-18))
                            .collect();
                        tracing::info!(update_hz, ?debts);
                        last_log = now;
                    }

                    let _ = db.tx.send(snapshot);
                    last_snapshot = now;
                }
            }
        });
        tx
    }

    fn update(&mut self, update: Update, now: DateTime<Utc>) {
        if update.timestamp < (now - self.window) {
            return;
        }
        let entry = self
            .data
            .entry(update.indexer)
            .or_default()
            .entry(hourly_timestamp(update.timestamp))
            .or_default();
        *entry += update.fee;
    }

    fn prune(&mut self, now: DateTime<Utc>) {
        let min_timestamp = hourly_timestamp(now - self.window);
        self.data.retain(|_, entries| {
            entries.retain(|t, _| *t > min_timestamp);
            !entries.is_empty()
        });
    }

    fn snapshot(&self) -> BTreeMap<Address, u128> {
        self.data
            .iter()
            .map(|(indexer, entries)| (*indexer, entries.values().sum()))
            .collect()
    }
}

fn hourly_timestamp(t: DateTime<Utc>) -> i64 {
    let t = t.timestamp();
    t - (t % Duration::hours(1).num_seconds())
}
