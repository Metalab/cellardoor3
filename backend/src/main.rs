use std::{
    collections::HashSet,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Context;
use clap::Parser;
use dashmap::DashSet;
use mio::{Events, Interest, Token};
use udev::MonitorBuilder;

mod config;

const W1_TOKEN: Token = Token(0);

type OneWireId = [u8; 7];

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

    let access_list = Arc::new(
        deserialize_1w_devices(&config.persistence.path).unwrap_or_else(|e| {
            log::error!("Failed to deserialize persisted key list, using empty list: {e:?}");
            DashSet::new()
        }),
    );
    let inner_access_list = access_list.clone();

    std::thread::spawn(move || loop {
        match mos_refresh(&config.thing, &config.persistence, &inner_access_list) {
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
                        match parse_1w_id(event.sysname().to_str().unwrap()) {
                            Ok(id) => {
                                if access_list.contains(&id) {
                                    log::info!("Valid user detected!");
                                } else {
                                    log::debug!("Invalid user detected!");
                                }
                            }
                            Err(e) => {
                                log::warn!("Failed to parse device id: {e:?}");
                            }
                        }
                    });
            }
        }
    }
}

fn mos_refresh(
    config: &config::Thing,
    persistence: &config::Persistence,
    access_list: &Arc<DashSet<OneWireId>>,
) -> anyhow::Result<()> {
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
                            match parse_1w_id(id) {
                                Ok(id) => {
                                    ids.insert(id);
                                }
                                Err(e) => {
                                    log::error!("Failed to parse ID {id:?}: {e:?}");
                                }
                            }
                        }
                    }
                    let len = ids.len();
                    let old_len = access_list.len();
                    access_list.retain(|button| ids.remove(button));
                    log::debug!(
                        "List of IDs refreshed, we have {len} buttons now ({} new, {} removed)",
                        ids.len(),
                        old_len - access_list.len(),
                    );
                    let updated = !ids.is_empty() || old_len - access_list.len() > 0;
                    for id in ids {
                        access_list.insert(id);
                    }

                    if updated {
                        if let Err(err) = serialize_1w_devices(access_list, &persistence.path) {
                            log::error!("Failed to persist key list: {err:?}");
                        }
                    }
                }
            }
            Err(e) => {
                log::error!("Failed fetching key list: {e:?}");
            }
        }

        std::thread::sleep(std::time::Duration::from_secs(config.refresh_secs));
    }
}

fn parse_1w_id(id: &str) -> anyhow::Result<[u8; 7]> {
    let (devtype, id) = id.split_once('-').context("Wrong id format")?;

    let mut result = [0u8; 7];
    result[0] = u8::from_str_radix(devtype, 16)?;
    for idx in 0..6 {
        result[idx + 1] = u8::from_str_radix(&id[(idx * 2)..(idx * 2 + 2)], 16)?;
    }

    Ok(result)
}

fn serialize_1w_devices(
    list: &DashSet<OneWireId>,
    destination: impl AsRef<Path>,
) -> anyhow::Result<()> {
    let mut file = File::create(destination)?;
    for id in list.iter() {
        file.write_all(&*id)?;
    }
    file.flush()?;

    Ok(())
}

fn deserialize_1w_devices(destination: impl AsRef<Path>) -> anyhow::Result<DashSet<OneWireId>> {
    let mut file = File::open(destination)?;

    let set = DashSet::new();

    let mut id = OneWireId::default();
    loop {
        match file.read_exact(&mut id) {
            Ok(_) => {
                set.insert(id);
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    }

    Ok(set)
}

mod test {
    #[test]
    fn parse_1w_id_test() {
        let id = "33-00000392c6ea";
        let id_bytes = super::parse_1w_id(id).unwrap();
        assert_eq!(id_bytes, [0x33, 0x00, 0x00, 0x03, 0x92, 0xc6, 0xea]);
    }
}
