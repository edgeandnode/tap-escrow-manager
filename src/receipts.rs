use std::collections::BTreeMap;

use alloy::{hex::ToHexExt as _, primitives::Address};
use chrono::{DateTime, Duration, Utc};
use futures_util::StreamExt as _;
use prost::Message as _;
use rdkafka::{
    consumer::{Consumer, StreamConsumer},
    Message,
};
use tokio::sync::{mpsc, watch};

use crate::config;

pub async fn track_receipts(
    config: &config::Kafka,
    signers: Vec<Address>,
) -> anyhow::Result<watch::Receiver<BTreeMap<Address, u128>>> {
    let window = Duration::days(28);
    let (tx, rx) = watch::channel(Default::default());
    let db = DB::spawn(window, tx);

    let mut client_config = rdkafka::ClientConfig::new();
    client_config.extend(config.config.clone().into_iter());
    let defaults = [
        ("group.id", "tap-escrow-manager"),
        ("enable.auto.commit", "true"),
        ("auto.offset.reset", "earliest"),
    ];
    for (key, value) in defaults {
        if !client_config.config_map().contains_key(key) {
            client_config.set(key, value);
        }
    }
    let mut consumer: StreamConsumer = client_config.create()?;
    consumer.subscribe(&[&config.realtime_topic])?;

    tokio::spawn(async move {
        if let Err(kafka_consumer_err) = process_messages(&mut consumer, db, signers).await {
            tracing::error!(%kafka_consumer_err);
        }
    });

    Ok(rx)
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
            #[derive(prost::Message)]
            struct Payload {
                /// 20 bytes (address)
                #[prost(bytes, tag = "1")]
                signer: Vec<u8>,
                /// 20 bytes (address)
                #[prost(bytes, tag = "2")]
                receiver: Vec<u8>,
                #[prost(double, tag = "3")]
                fee_grt: f64,
            }
            let payload = match Payload::decode(payload) {
                Ok(payload) => payload,
                Err(payload_parse_err) => {
                    tracing::error!(%payload_parse_err, input = payload.encode_hex());
                    return;
                }
            };
            if !signers.contains(&Address::from_slice(&payload.signer)) {
                return;
            }
            let update = Update {
                timestamp,
                indexer: Address::from_slice(&payload.receiver),
                fee: (payload.fee_grt * 1e18) as u128,
            };
            let _ = db.send(update).await;
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
