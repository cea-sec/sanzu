use anyhow::{Context, Result};

#[macro_use]
extern crate log;

use clap::CommandFactory;
use twelf::Layer;

use sanzu::{
    config::read_server_config,
    server,
    utils::{ServerArgs, ServerArgsConfig},
};

use sanzu_common::proto::VERSION;

fn main() -> Result<()> {
    env_logger::Builder::from_default_env()
        .format_timestamp_nanos()
        .init();

    let matches = ServerArgs::command().get_matches();
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

    let server_config = ServerArgsConfig::with_layers(&layers).unwrap();

    if server_config.proto {
        println!("Protocol version: {VERSION}");
        return Ok(());
    }
    let conf =
        read_server_config(&server_config.config).context("Cannot read configuration file")?;
    if let Err(err) = server::run(&conf, &server_config) {
        error!("Server error");
        err.chain().for_each(|cause| error!(" - due to {}", cause));
    }
    Ok(())
}
