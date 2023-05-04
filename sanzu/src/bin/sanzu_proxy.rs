use anyhow::{Context, Result};
use clap::CommandFactory;

#[macro_use]
extern crate log;

use sanzu::{
    config::read_server_config,
    proxy,
    utils::{ProxyArgs, ProxyArgsConfig},
};
use sanzu_common::proto::VERSION;

use twelf::Layer;

fn main() -> Result<()> {
    env_logger::Builder::from_default_env().init();

    let matches = ProxyArgs::command().get_matches();
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

    let proxy_config = ProxyArgsConfig::with_layers(&layers).unwrap();

    if proxy_config.proto {
        println!("Protocol version: {VERSION}");
        return Ok(());
    }
    let conf =
        read_server_config(&proxy_config.config).context("Cannot read configuration file")?;
    if let Err(err) = proxy::run(&conf, &proxy_config) {
        error!("Proxy error");
        err.chain().for_each(|cause| error!(" - due to {}", cause));
    }
    Ok(())
}
