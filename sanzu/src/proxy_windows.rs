use crate::{config::ConfigServer, utils::ProxyArgsConfig};
use anyhow::Result;

pub fn run(_config: &ConfigServer, _arguments: &ProxyArgsConfig) -> Result<()> {
    panic!("Unsupported os");
}
