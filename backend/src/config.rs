use std::{fs::read, path::Path};

#[derive(serde::Deserialize, Debug)]
pub struct Thing {
    pub url: String,
    pub token: String,
    pub refresh_secs: u64,
}

#[derive(serde::Deserialize, Debug)]
pub struct Config {
    pub thing: Thing,
    pub logging: log4rs::config::RawConfig,
}

impl Config {
    pub fn parse(path: impl AsRef<Path>) -> anyhow::Result<Config> {
        Ok(serde_yaml_ng::from_slice(&read(path)?)?)
    }
}
