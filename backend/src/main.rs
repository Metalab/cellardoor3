use std::{collections::HashSet, path::PathBuf, sync::Arc};

use anyhow::Context;
use clap::Parser;
use dashmap::DashSet;
use mio::{Events, Interest, Token};
use udev::MonitorBuilder;

mod config;

const W1_TOKEN: Token = Token(0);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct IButton {
    name: String,
}

impl From<String> for IButton {
    fn from(name: String) -> Self {
        Self { name }
    }
}

#[derive(Parser, Debug)]
struct Args {
    #[clap(short = 'c', long, default_value = "config.yaml", env)]
    config: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let config = config::Config::parse(&args.config)
        .context(format!("Failed to read file {:?}", args.config))?;
    log4rs::init_raw_config(config.logging)?;

    let access_list = Arc::new(DashSet::<String>::new());
    let inner_access_list = access_list.clone();

    std::thread::spawn(move || loop {
        match mos_refresh(&config.thing, &inner_access_list) {
            Ok(_) => {
                log::error!("MOS refresh thread terminated");
            }
            Err(e) => {
                log::error!("MOS refresh thread error: {:?}", e);
            }
        }
    });

    let monitor = MonitorBuilder::new()?.match_subsystem("w1")?;
    let mut socket = monitor.listen()?;

    let mut poll = mio::Poll::new()?;
    let mut events = Events::with_capacity(1024);
    poll.registry()
        .register(&mut socket, W1_TOKEN, Interest::READABLE)?;

    loop {
        poll.poll(&mut events, None)?;

        for event in &events {
            if event.token() == W1_TOKEN {
                socket
                    .iter()
                    .filter(|event| event.event_type() == udev::EventType::Add)
                    .for_each(|event| {
                        log::debug!("device recognized: {:?}", event.sysname());
                        if access_list.contains(event.sysname().to_str().unwrap()) {
                            log::info!("Valid user detected!");
                        }
                    });
            }
        }
    }
}

fn mos_refresh(config: &config::Thing, access_list: &Arc<DashSet<String>>) -> anyhow::Result<()> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("X-TOKEN", config.token.parse().unwrap());
    let client = reqwest::blocking::Client::builder()
        .default_headers(headers)
        .build()?;
    loop {
        match client.get(&config.url).send() {
            Ok(resp) => {
                if resp.status().is_success() {
                    let mut ids = HashSet::new();
                    for line in resp.text().unwrap().lines() {
                        let line = line.trim();
                        if line.is_empty() || line.starts_with('#') {
                            continue;
                        }
                        if let Some((id, _name)) = line.split_once(',') {
                            ids.insert(id.to_string());
                        }
                    }
                    let len = ids.len();
                    let old_len = access_list.len();
                    access_list.retain(|button| ids.remove(button));
                    log::debug!(
                        "List of IDs refreshed, we have {} buttons now ({} new, {} removed)",
                        len,
                        ids.len(),
                        old_len - access_list.len(),
                    );
                    for id in ids {
                        access_list.insert(id);
                    }
                }
            }
            Err(e) => {
                log::error!("Failed fetching key list: {:?}", e);
            }
        }

        std::thread::sleep(std::time::Duration::from_secs(config.refresh_secs));
    }
}
