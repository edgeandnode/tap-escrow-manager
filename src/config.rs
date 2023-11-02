use std::{fmt, ops::Deref};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub kafka: Kafka,
}

#[derive(Debug, Deserialize)]
pub struct Kafka {
    pub bootstrap_servers: Hidden<String>,
    pub ca_location: String,
    pub sasl_username: String,
    pub sasl_password: Hidden<String>,
    pub topic: String,
}

#[derive(Deserialize)]
#[serde(transparent)]
pub struct Hidden<T>(T);

impl<T: fmt::Debug> fmt::Debug for Hidden<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HIDDEN")
    }
}

impl<T> Deref for Hidden<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
