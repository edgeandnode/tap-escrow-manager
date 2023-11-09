use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::path::PathBuf;

use alloy_primitives::Address;
use anyhow::Context as _;
use chrono::Duration;
use chrono::{serde::ts_milliseconds, DateTime, Utc};
use eventuals::{Eventual, EventualWriter, Ptr};
use rdkafka::TopicPartitionList;
use rdkafka::{
    consumer::{Consumer, StreamConsumer},
    Message,
};
use serde::{Deserialize, Serialize};

use crate::config;

pub async fn track_receipts(
    config: &config::Kafka,
) -> anyhow::Result<Eventual<Ptr<HashMap<Address, u128>>>> {
    let window = Duration::days(28);
    let db = DB::new(config.checkpoint_file.clone(), window).context("failed to init DB")?;
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

    let timeout = std::time::Duration::from_secs(10);
    let partitions = consumer
        .fetch_metadata(Some(&config.topic), timeout)?
        .topics()[0]
        .partitions()
        .len();
    let mut assignment = TopicPartitionList::new();
    for partition in 0..partitions {
        assignment.add_partition(&config.topic, partition as i32);
    }
    consumer.assign(&assignment)?;
    let posisitons =
        consumer.offsets_for_timestamp(latest_timestamp.timestamp_millis(), timeout)?;
    for position in posisitons.elements() {
        assignment.set_partition_offset(&config.topic, position.partition(), position.offset())?;
    }
    consumer.seek_partitions(assignment, timeout)?;

    let (tx, rx) = Eventual::new();
    tokio::spawn(async move {
        if let Err(kafka_consumer_err) = process_messages(&mut consumer, db, tx).await {
            tracing::error!(%kafka_consumer_err);
        }
    });
    Ok(rx)
}

async fn process_messages(
    consumer: &mut StreamConsumer,
    mut db: DB,
    mut tx: EventualWriter<Ptr<HashMap<Address, u128>>>,
) -> anyhow::Result<()> {
    let mut rate_count: u64 = 0;
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

        #[derive(Deserialize)]
        struct Payload {
            #[serde(with = "ts_milliseconds")]
            timestamp: DateTime<Utc>,
            indexer: Address,
            fee: f64,
        }
        let payload: Payload = serde_json::from_reader(payload)?;
        let fees = (payload.fee * 1e18) as u128;
        db.update(payload.indexer, payload.timestamp, fees);

        if Utc::now().signed_duration_since(db.last_flush) > Duration::seconds(30) {
            let msg_hz = rate_count / 30;
            rate_count = 0;
            tracing::info!(timestamp = ?payload.timestamp, msg_hz, "checkpoint");
            db.flush()?;
            tx.write(Ptr::new(db.total_fees()));
        }
    }
}

struct DB {
    /// indexer -> (date -> total_fees)
    data: BTreeMap<Address, Vec<(DateTime<Utc>, u128)>>,
    file: PathBuf,
    window: Duration,
    last_flush: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Record {
    timestamp: DateTime<Utc>,
    indexer: Address,
    fees: u128,
}

impl DB {
    fn new(file: PathBuf, window: Duration) -> anyhow::Result<Self> {
        let _ = File::options().create_new(true).write(true).open(&file);
        let mut reader = csv::Reader::from_path(&file)?;
        let start = Utc::now() - window;
        let records: Vec<Record> = reader
            .deserialize()
            .filter(|r| {
                r.as_ref()
                    .map(|r: &Record| r.timestamp >= start)
                    .unwrap_or(true)
            })
            .collect::<Result<_, _>>()?;
        let last_flush = records.iter().map(|r| r.timestamp).max().unwrap_or(start);
        let mut this = Self {
            data: Default::default(),
            file,
            window,
            last_flush,
        };
        for record in records {
            this.update(record.indexer, record.timestamp, record.fees);
        }
        Ok(this)
    }

    fn flush(&mut self) -> anyhow::Result<()> {
        let mut writer = csv::Writer::from_path(&self.file)?;
        let now = Utc::now();
        for (indexer, daily_fees) in &mut self.data {
            // prune old entries
            daily_fees.retain(|(t, _)| t >= &(now - self.window));
            // copy & sort for easier debugging
            let mut daily_fees = daily_fees.clone();
            daily_fees.sort_by_key(|(t, _)| *t);

            for (timestamp, fees) in daily_fees {
                writer.serialize(Record {
                    timestamp,
                    indexer: *indexer,
                    fees,
                })?;
            }
        }
        writer.flush()?;
        self.last_flush = Utc::now();
        Ok(())
    }

    fn update(&mut self, indexer: Address, timestamp: DateTime<Utc>, fees: u128) {
        if timestamp < (Utc::now() - self.window) {
            return;
        }
        let daily_fees = self.data.entry(indexer).or_default();
        let index = daily_fees
            .iter_mut()
            .position(|(t, _)| t.date_naive() == timestamp.date_naive());
        match index {
            None => {
                daily_fees.push((timestamp, fees));
            }
            Some(index) => {
                daily_fees[index] = (timestamp, daily_fees[index].1 + fees);
            }
        }
    }

    fn total_fees(&self) -> HashMap<Address, u128> {
        self.data
            .iter()
            .map(|(indexer, daily_fees)| {
                let total_fees = daily_fees.iter().map(|(_, fees)| fees).sum();
                (*indexer, total_fees)
            })
            .collect()
    }
}
