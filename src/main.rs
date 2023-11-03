use std::{env, fs};

use anyhow::{anyhow, Context as _};
use config::Config;

use crate::receipts::track_receipts;

mod config;
mod receipts;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config_file = env::args()
        .into_iter()
        .nth(1)
        .ok_or_else(|| anyhow!("missing config file argument"))?;
    let config: Config =
        serde_json::from_str(&fs::read_to_string(config_file).context("failed to load config")?)?;
    tracing::info!("{config:#?}");

    let debts = track_receipts(&config.kafka)
        .await
        .context("failed to start kafka client")?;
    println!("{:#?}", debts.value().await.unwrap().as_ref());

    Ok(())
}
