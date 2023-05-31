#![windows_subsystem = "windows"]
use anyhow::Result;
use clap::CommandFactory;

#[macro_use]
extern crate log;

use sanzu::{
    client,
    config::{read_client_config, ConfigClient},
    utils::{init_logger, is_proto_arg, ClientArgs, ClientArgsConfig},
};

use sanzu_common::proto::VERSION;

use std::collections::HashMap;

use twelf::Layer;

#[cfg(windows)]
use winapi::um::wincon;

fn main() -> Result<()> {
    #[cfg(windows)]
    {
        unsafe {
            wincon::AttachConsole(wincon::ATTACH_PARENT_PROCESS);
        }
    }

    if is_proto_arg() {
        println!("Protocol version: {VERSION}");
        return Ok(());
    }

    let matches = ClientArgs::command().get_matches();
    let args_config = matches.get_one::<std::path::PathBuf>("args_config");

    let mut layers = if let Some(args_config) = args_config {
        vec![Layer::Toml(args_config.into())]
    } else {
        vec![]
    };
    layers.append(&mut vec![
        Layer::Env(Some(String::from("SANZU_"))),
        Layer::Clap(matches),
    ]);

    let client_config = ClientArgsConfig::with_layers(&layers).unwrap();

    init_logger(client_config.verbose);

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
