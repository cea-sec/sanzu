use serde::{Deserialize, Serialize};
use std::{fs::File, io, io::Read, path::Path};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Tls {
    pub server_name: String,
    pub ca_file: String,
    pub crl_file: Option<String>,
    pub ocsp_file: Option<String>,
    pub auth_cert: String,
    pub auth_key: String,
    /// List of domains to authenticate clients
    pub allowed_client_domains: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Command {
    pub command_bin: String,
    pub command_args: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CmdCallBack {
    pub on_connect: Command,
}

/// Support authentication mecanism
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", content = "args")]
pub enum AuthType {
    #[cfg(all(unix, feature = "kerberos"))]
    // List of allowed realms
    Kerberos(Vec<String>),
    // Pam name
    Pam(String),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub tls: Tls,
    pub auth_type: Option<AuthType>,
    pub cmd_callback: CmdCallBack,
}

pub fn read_config<P: AsRef<Path>>(path: P) -> io::Result<Config> {
    let mut content = String::new();
    File::open(path)?.read_to_string(&mut content)?;
    toml::from_str(&content).map_err(|err| io::Error::new(io::ErrorKind::Other, err))
}
