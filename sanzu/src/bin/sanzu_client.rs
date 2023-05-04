#![windows_subsystem = "windows"]
use anyhow::Result;

#[macro_use]
extern crate log;

use clap::CommandFactory;

use sanzu::{
    client,
    config::{read_client_config, ConfigClient},
    utils::{ClientArgs, ClientArgsConfig},
};

use sanzu_common::proto::VERSION;

use std::collections::HashMap;

use twelf::Layer;

#[cfg(windows)]
use winapi::um::wincon;

fn main() -> Result<()> {
    env_logger::Builder::from_default_env()
        .format_timestamp_nanos()
        .init();

    #[cfg(windows)]
    {
        unsafe {
            wincon::AttachConsole(wincon::ATTACH_PARENT_PROCESS);
        }
    }

    let matches = ClientArgs::command().get_matches();
    let config_path = matches.get_one::<std::path::PathBuf>("config_path");

    let mut layers = if let Some(config_path) = config_path {
        vec![Layer::Toml(config_path.into())]
    } else {
        vec![]
    };
    layers.append(&mut vec![
        Layer::Env(Some(String::from("SANZU_"))),
        Layer::Clap(matches),
    ]);

    let client_config = ClientArgsConfig::with_layers(&layers).unwrap();

    if client_config.proto {
        println!("Protocol version: {VERSION}");
        return Ok(());
    }

    let conf = match client_config.config {
        Some(ref client_config) => {
            read_client_config(client_config).expect("Cannot read configuration file")
        }
        None => ConfigClient {
            ffmpeg: HashMap::new(),
        },
    };
    if let Err(err) = client::run(
        &conf,
        &client_config,
        client::StdioClientInterface::default(),
    ) {
        error!("Client error");
        err.chain().for_each(|cause| error!(" - due to {}", cause));
    }
    Ok(())
}
