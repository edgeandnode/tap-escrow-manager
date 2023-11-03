use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::{path::PathBuf, time::Duration};

use alloy_primitives::Address;
use anyhow::Context as _;
use chrono::{serde::ts_milliseconds, DateTime, Utc};
use eventuals::{Eventual, EventualWriter, Ptr};
use rdkafka::{
    consumer::{Consumer, StreamConsumer},
    Message,
};
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use crate::config;

pub async fn track_receipts(
    config: &config::Kafka,
) -> anyhow::Result<Eventual<Ptr<HashMap<Address, u128>>>> {
    let db = DB::new(config.checkpoint_file.clone()).context("failed to init DB")?;

    // TODO: move partition 0 cursor to db.last_flush or 28 days ago

    let mut consumer: StreamConsumer = rdkafka::ClientConfig::new()
        .set("group.id", "gw-escrow-manager")
        .set("bootstrap.servers", config.bootstrap_servers.clone())
        .set("security.protocol", "sasl_ssl")
        .set("sasl.mechanism", "SCRAM-SHA-256")
        .set("sasl.username", config.sasl_username.clone())
        .set("sasl.password", config.sasl_password.clone())
        .set("ssl.ca.location", config.ca_location.clone())
        .create()?;
    consumer.subscribe(&[&config.topic])?;

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
    let mut last_report = Instant::now();
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

        #[derive(Deserialize)]
        struct Payload {
            #[serde(with = "ts_milliseconds")]
            timestamp: DateTime<Utc>,
            indexer: Address,
            fee: f64,
        }
        let payload: Payload = serde_json::from_reader(payload)?;
        db.update(
            payload.indexer,
            payload.timestamp,
            (payload.fee * 1e18) as u128,
        );

        let now = Instant::now();
        if now.saturating_duration_since(last_report) > Duration::from_secs(2 /* TODO: 30s */) {
            last_report = now;
            tracing::info!(timestamp = %payload.timestamp, "checkpoint");
            db.flush()?;
            tx.write(Ptr::new(db.total_fees()));
        }

        break;
    }
    anyhow::bail!("TODO");
}

struct DB {
    /// indexer -> (date -> total_fees)
    data: BTreeMap<Address, Vec<(DateTime<Utc>, u128)>>,
    file: PathBuf,
    last_flush: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Record {
    #[serde(with = "ts_milliseconds")]
    timestamp: DateTime<Utc>,
    indexer: Address,
    fees: u128,
}

impl DB {
    fn new(file: PathBuf) -> anyhow::Result<Self> {
        let _ = File::options().create_new(true).write(true).open(&file);
        let mut reader = csv::Reader::from_path(&file)?;
        let records: Vec<Record> = reader.deserialize().collect::<Result<_, _>>()?;
        let last_flush = records
            .iter()
            .map(|r| r.timestamp)
            .max()
            .unwrap_or_default();
        let mut this = Self {
            data: Default::default(),
            file,
            last_flush,
        };
        for record in records {
            this.update(record.indexer, record.timestamp, record.fees);
        }
        Ok(this)
    }

    fn flush(&mut self) -> anyhow::Result<()> {
        let mut writer = csv::Writer::from_path(&self.file)?;
        for (indexer, daily_fees) in &self.data {
            for (timestamp, fees) in daily_fees {
                writer.serialize(Record {
                    timestamp: timestamp.clone(),
                    indexer: indexer.clone(),
                    fees: *fees,
                })?;
            }
        }
        writer.flush()?;
        self.last_flush = Utc::now();
        Ok(())
    }

    fn update(&mut self, indexer: Address, timestamp: DateTime<Utc>, fees: u128) {
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
                (indexer.clone(), total_fees)
            })
            .collect()
    }
}
