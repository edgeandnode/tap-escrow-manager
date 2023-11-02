use std::collections::HashMap;

use alloy_primitives::Address;
use eventuals::{Eventual, EventualWriter, Ptr};
use rdkafka::{
    consumer::{Consumer, StreamConsumer},
    error::KafkaError,
};

use crate::config;

pub async fn track_receipts(
    config: &config::Kafka,
) -> Result<Eventual<Ptr<HashMap<Address, u64>>>, KafkaError> {
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
        if let Err(kafka_consumer_err) = process_messages(&mut consumer, tx).await {
            tracing::error!(%kafka_consumer_err);
        }
    });
    Ok(rx)
}

async fn process_messages(
    consumer: &mut StreamConsumer,
    tx: EventualWriter<Ptr<HashMap<Address, u64>>>,
) -> anyhow::Result<()> {
    loop {
        let msg = match consumer.recv().await {
            Ok(msg) => msg,
            Err(recv_error) => {
                tracing::error!(%recv_error);
                continue;
            }
        };
        todo!();
    }
}
