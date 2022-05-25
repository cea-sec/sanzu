use crate::{config::ConfigServer, utils::ArgumentsProxy};
use anyhow::Result;

pub fn run(_config: &ConfigServer, _arguments: &ArgumentsProxy) -> Result<()> {
    panic!("Unsupported os");
}
