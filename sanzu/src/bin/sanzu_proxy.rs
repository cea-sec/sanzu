use anyhow::{Context, Result};
use clap::CommandFactory;

#[macro_use]
extern crate log;

use sanzu::{
    config::read_server_config,
    proxy,
    utils::{init_logger, is_proto_arg, ProxyArgs, ProxyArgsConfig},
};

use sanzu_common::proto::VERSION;

use twelf::Layer;

fn main() -> Result<()> {
    if is_proto_arg() {
        println!("Protocol version: {VERSION}");
        return Ok(());
    }

    let matches = ProxyArgs::command().get_matches();
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

    let proxy_config = ProxyArgsConfig::with_layers(&layers).unwrap();

    init_logger(proxy_config.verbose);

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
