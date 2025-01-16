use std::collections::BTreeMap;

use alloy::primitives::Address;
use tokio::sync::watch;

use crate::config;

pub async fn track_receipts(
    _config: &config::Kafka,
    _signers: Vec<Address>,
) -> anyhow::Result<watch::Receiver<BTreeMap<Address, u128>>> {
    todo!()
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
