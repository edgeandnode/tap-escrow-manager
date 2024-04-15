use crate::config;
use anyhow::Context as _;
use chrono::{serde::ts_milliseconds, DateTime, Utc};
use chrono::{Datelike, Duration};
use eventuals::{Eventual, EventualWriter, Ptr};
use rdkafka::{
    consumer::{Consumer, StreamConsumer},
    Message,
};
use serde::Deserialize;
use std::io::Write as _;
use std::{collections::HashMap, fs::File, path::PathBuf};
use thegraph_core::types::alloy_primitives::Address;

pub async fn track_receipts(
    config: &config::Kafka,
    graph_env: String,
) -> anyhow::Result<Eventual<Ptr<HashMap<Address, u128>>>> {
    let window = Duration::days(28);
    let db = DB::new(config.cache.clone(), window).context("failed to init DB")?;
    let latest_timestamp = db.last_flush;
    tracing::debug!(?latest_timestamp);

    let mut client_config = rdkafka::ClientConfig::new();
    client_config.extend(config.config.clone().into_iter());
    let defaults = [
        ("group.id", "gw-escrow-manager"),
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

    let (tx, rx) = Eventual::new();
    tokio::spawn(async move {
        if let Err(kafka_consumer_err) = process_messages(&mut consumer, db, tx, graph_env).await {
            tracing::error!(%kafka_consumer_err);
        }
    });
    Ok(rx)
}

async fn process_messages(
    consumer: &mut StreamConsumer,
    mut db: DB,
    mut tx: EventualWriter<Ptr<HashMap<Address, u128>>>,
    graph_env: String,
) -> anyhow::Result<()> {
    let mut rate_count: u64 = 0;
    let mut latest_msg = Utc::now();
    loop {
        let msg = match consumer.recv().await {
            Ok(msg) => msg,
            Err(recv_error) => {
                tracing::error!(%recv_error);
                continue;
            }
        };
        let payload = match msg.payload() {
            Some(payload) => payload,
            None => continue,
        };

        rate_count += 1;
        if Utc::now().signed_duration_since(db.last_flush) > Duration::seconds(30) {
            let msg_hz = rate_count / 30;
            rate_count = 0;
            tracing::info!(?latest_msg, msg_hz, "flush");
            db.flush()?;
            tx.write(Ptr::new(db.total_fees()));
        }

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
        let payload: Payload = serde_json::from_reader(payload)?;
        latest_msg = payload.timestamp.max(latest_msg);
        if payload.legacy_scalar || (payload.graph_env != graph_env) {
            continue;
        }

        let fees = (payload.fee * 1e18) as u128;
        db.update(payload.indexer, payload.timestamp, fees);
    }
}

struct DB {
    data: HashMap<Address, Vec<u128>>,
    file: PathBuf,
    window: Duration,
    last_flush: DateTime<Utc>,
}

impl DB {
    fn new(file: PathBuf, window: Duration) -> anyhow::Result<Self> {
        let cache = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&file)
            .context("open cache file")?;
        let modified: DateTime<Utc> = DateTime::from(cache.metadata()?.modified()?);
        let mut data: HashMap<Address, Vec<u128>> =
            serde_json::from_reader(&cache).unwrap_or_default();
        drop(cache);
        assert!(data.values().all(|v| v.len() == window.num_days() as usize));
        let now = Utc::now();
        let offset: usize = (now - modified)
            .num_days()
            .min(window.num_days())
            .try_into()
            .unwrap_or(0);
        for bins in data.values_mut() {
            bins.rotate_right(offset);
            for entry in bins.iter_mut().take(offset) {
                *entry = 0;
            }
        }
        Ok(Self {
            data,
            file,
            window,
            last_flush: now,
        })
    }

    fn flush(&mut self) -> anyhow::Result<()> {
        let now = Utc::now();
        if self.last_flush.day() != now.day() {
            for bins in self.data.values_mut() {
                bins.rotate_right(1);
                bins[0] = 0;
            }
        }

        let mut file = File::create(&self.file)?;
        serde_json::to_writer(&file, &self.data)?;
        file.flush()?;

        self.last_flush = now;
        Ok(())
    }

    fn update(&mut self, indexer: Address, timestamp: DateTime<Utc>, fees: u128) {
        let now = Utc::now();
        if timestamp < (now - self.window) {
            tracing::warn!(
                "discaring update outside window, try moving up consumer group partition offsets"
            );
            return;
        }
        let daily_fees = self
            .data
            .entry(indexer)
            .or_insert_with(|| Vec::from_iter((0..self.window.num_days()).map(|_| 0)));
        let offset: usize = (now - timestamp).num_days().try_into().unwrap_or(0);
        daily_fees[offset] += fees;
    }

    fn total_fees(&self) -> HashMap<Address, u128> {
        self.data
            .iter()
            .map(|(indexer, daily_fees)| (*indexer, daily_fees.iter().sum()))
            .collect()
    }
}
