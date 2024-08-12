use std::{collections::BTreeMap, fs::File, io::Write as _, path::PathBuf};

use anyhow::Context as _;
use chrono::{serde::ts_milliseconds, DateTime, Duration, Utc};
use ethers::providers::StreamExt;
use flate2::{read::GzDecoder, write::GzEncoder};
use rdkafka::{
    consumer::{Consumer, StreamConsumer},
    Message,
};
use serde::Deserialize;
use thegraph_core::types::alloy_primitives::Address;
use tokio::sync::{mpsc, watch};

use crate::config;

pub async fn track_receipts(
    config: &config::Kafka,
    graph_env: String,
) -> anyhow::Result<watch::Receiver<BTreeMap<Address, u128>>> {
    let window = Duration::days(28);
    let (tx, rx) = watch::channel(Default::default());
    let db = DB::spawn(config.cache.clone(), window, tx).context("failed to init DB")?;

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
    consumer.subscribe(&[&config.topic])?;

    tokio::spawn(async move {
        if let Err(kafka_consumer_err) = process_messages(&mut consumer, db, graph_env).await {
            tracing::error!(%kafka_consumer_err);
        }
    });

    Ok(rx)
}

async fn process_messages(
    consumer: &mut StreamConsumer,
    db: mpsc::Sender<Update>,
    graph_env: String,
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
            #[derive(Deserialize)]
            struct Payload {
                #[serde(with = "ts_milliseconds")]
                timestamp: DateTime<Utc>,
                graph_env: String,
                indexer: Address,
                fee: f64,
                #[serde(default)]
                legacy_scalar: bool,
            }
            let payload: Payload = match serde_json::from_reader(payload) {
                Ok(payload) => payload,
                Err(payload_parse_err) => {
                    tracing::error!(%payload_parse_err, input = %String::from_utf8_lossy(payload));
                    return;
                }
            };
            if payload.legacy_scalar || (payload.graph_env != graph_env) {
                return;
            }
            let update = Update {
                timestamp: payload.timestamp,
                indexer: payload.indexer,
                fee: (payload.fee * 1e18) as u128,
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
    file: PathBuf,
    window: Duration,
    tx: watch::Sender<BTreeMap<Address, u128>>,
}

impl DB {
    pub fn spawn(
        file: PathBuf,
        window: Duration,
        tx: watch::Sender<BTreeMap<Address, u128>>,
    ) -> anyhow::Result<mpsc::Sender<Update>> {
        let cache = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&file)
            .context("open cache file")?;
        let data = match serde_json::from_reader(GzDecoder::new(&cache)) {
            Ok(data) => data,
            Err(cache_err) => {
                tracing::error!(%cache_err);
                Default::default()
            }
        };
        drop(cache);
        let mut db = DB {
            data,
            file,
            window,
            tx,
        };
        db.prune(Utc::now());

        let (tx, mut rx) = mpsc::channel(128);
        tokio::spawn(async move {
            let mut last_flush = Utc::now();
            let mut last_snapshot = Utc::now();
            let mut message_total: usize = 0;
            let buffer_size = 128;
            let mut buffer: Vec<Update> = Vec::with_capacity(buffer_size);
            loop {
                rx.recv_many(&mut buffer, buffer_size).await;
                let now = Utc::now();
                message_total += buffer.len();
                for update in buffer.drain(..) {
                    db.update(update, now);
                }

                if (now - last_snapshot) >= Duration::seconds(1) {
                    let _ = db.tx.send(db.snapshot());
                    last_snapshot = now;
                }
                if (now - last_flush) >= Duration::minutes(1) {
                    db.prune(now);
                    if let Err(flush_err) = db.flush() {
                        tracing::error!(%flush_err);
                    };
                    let debts: BTreeMap<Address, f64> = db
                        .snapshot()
                        .into_iter()
                        .map(|(k, v)| (k, v as f64 * 1e-18))
                        .collect();
                    let update_hz = message_total / (now - last_flush).num_seconds() as usize;
                    tracing::info!(?debts, update_hz);
                    message_total = 0;
                    last_flush = now;
                }
            }
        });

        Ok(tx)
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

    fn flush(&mut self) -> anyhow::Result<()> {
        let mut file = File::create(&self.file)?;
        let gz = GzEncoder::new(file.try_clone().unwrap(), flate2::Compression::new(4));
        serde_json::to_writer(gz, &self.data)?;
        file.flush()?;
        Ok(())
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
