use std::collections::{BTreeMap, HashMap};
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

    // TODO: move partition 0 cursor to db.last_flush

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

        // TODO: deserialize msg, update in-memory DB record

        let now = Instant::now();
        if now.saturating_duration_since(last_report) > Duration::from_secs(2 /* TODO: 30s */) {
            if let Some(timestamp) = msg.timestamp().to_millis() {
                last_report = now;
                tracing::info!(timestamp, "checkpoint");
                db.flush()?;
                tx.write(Ptr::new(
                    db.data.values().map(|r| (r.indexer, r.fees_grt)).collect(),
                ));
            }
        }

        break;
    }
    anyhow::bail!("TODO");
}

struct DB {
    data: BTreeMap<Address, Record>,
    file: PathBuf,
    last_flush: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Record {
    #[serde(with = "ts_milliseconds")]
    timestamp: DateTime<Utc>,
    indexer: Address,
    fees_grt: u128,
}

impl DB {
    fn new(file: PathBuf) -> anyhow::Result<Self> {
        let mut reader = csv::Reader::from_path(&file)?;
        let records: Vec<Record> = reader.deserialize().collect::<Result<_, _>>()?;
        let last_flush = records
            .iter()
            .map(|r| r.timestamp)
            .max()
            .unwrap_or_default();
        let data = records.into_iter().map(|r| (r.indexer, r)).collect();
        Ok(Self {
            data,
            file,
            last_flush,
        })
    }

    fn flush(&mut self) -> anyhow::Result<()> {
        let mut writer = csv::Writer::from_path(&self.file)?;
        let now = Utc::now();
        for (_, record) in &mut self.data {
            record.timestamp = now;
            writer.serialize(record)?;
        }
        writer.flush()?;
        self.last_flush = now;
        Ok(())
    }
}
