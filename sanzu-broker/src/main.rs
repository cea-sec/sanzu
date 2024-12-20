#[macro_use]
extern crate anyhow;

#[macro_use]
extern crate sanzu_common;

use anyhow::{Context, Result};
#[macro_use]
extern crate log;

use clap::{Arg, Command};
mod config;

use config::{read_config, AuthType, Config};
use mio::{
    net::{TcpStream, UnixStream},
    Events, Interest, Poll, Token,
};

use nix::{
    sys::wait::waitpid,
    unistd::{fork, ForkResult},
};

use sanzu_common::{
    auth_pam::do_pam_auth,
    proto::{recv_client_msg_or_error, recv_server_msg_or_error, Tunnel, VERSION},
    tls_helper::{get_subj_alt_names, make_server_config, tls_do_handshake},
    tunnel,
    utils::get_username_from_principal,
};

#[cfg(all(unix, feature = "kerberos"))]
use sanzu_common::{auth_kerberos::do_kerberos_client_auth, proto::*};

use rustls::ServerConnection;

use std::{
    fs::remove_file,
    io::{Read, Write},
    net::{IpAddr, SocketAddr},
    process,
    time::Duration,
};

use uuid::Uuid;
use x509_parser::prelude::*;

const DEFAULT_CONFIG: &str = "sanzu_broker.toml";
const TOKEN_USERNAME: &str = "%USERNAME%";
const TOKEN_CLIENT_ADDR: &str = "%CLIENT_ADDR%";
const TOKEN_UNIX_SOCKET_PATH: &str = "%UNIX_SOCK_PATH%";

const SERVER: Token = Token(0);
const CLIENT: Token = Token(1);

/// Add tcp timeout
/// This can avoid application stalling on network outage
pub fn set_tcp_timeout(socket_ref: socket2::SockRef, timeout: Option<Duration>) -> Result<()> {
    if let Some(connection_timeout) = &timeout {
        info!("Set keep alive");
        let keepalive = socket2::TcpKeepalive::new()
            .with_time(*connection_timeout)
            .with_interval(*connection_timeout);

        socket_ref
            .set_tcp_keepalive(&keepalive)
            .context("Cannot set keepalive timeout")?;
        socket_ref
            .set_linger(Some(*connection_timeout))
            .context("Cannot set linger timeout")?;

        socket_ref
            .set_keepalive(true)
            .context("Cannot set keepalive")?;
        #[cfg(target_os = "linux")]
        socket_ref
            .set_tcp_user_timeout(Some(*connection_timeout))
            .context("Cannot set tcp user timeout")?;
    }
    Ok(())
}

/// Replace pattern tokens in list
pub fn replace_source(args: &[String], needle: &str, new_str: &str) -> Vec<String> {
    args.iter()
        .map(|arg| {
            if arg == needle {
                new_str.to_owned()
            } else {
                arg.to_owned()
            }
        })
        .collect()
}

/// Tls auth / Kerberos Auth
fn auth_client(
    config: &Config,
    mut socket: &mut std::net::TcpStream,
    addr: &SocketAddr,
) -> Result<(ServerConnection, String, tunnel::Version)> {
    socket.set_nodelay(true)?;

    let tls_config = make_server_config(
        &config.tls.ca_file,
        config.tls.crl_file.as_deref(),
        config.tls.ocsp_file.as_deref(),
        &config.tls.auth_cert,
        &config.tls.auth_key,
        config.tls.allowed_client_domains.is_some(),
    )
    .context("Cannot make tls config")?;
    debug!("Using tls");

    let mut tls_conn =
        ServerConnection::new(tls_config).context("Error in new ServerConnection")?;

    let mut username = None;
    if let Some(ref allowed_client_domains) = config.tls.allowed_client_domains {
        tls_do_handshake(&mut tls_conn, socket).context("Error in tls_do_handshake")?;
        let certs = tls_conn.peer_certificates();
        let certs = certs
            .map(Ok)
            .unwrap_or_else(|| Err(anyhow!("No cert from user")))?;
        let cert = certs
            .last()
            .map(Ok)
            .unwrap_or_else(|| Err(anyhow!("No cert from user")))?;
        let (_, cert) =
            X509Certificate::from_der(cert).context("Error in X509Certificate from der")?;

        let subj_alt_name =
            get_subj_alt_names(&cert).context("Error in get subject alternative name")?;
        debug!("Alt name: {:?}", subj_alt_name);

        let tls_username = get_username_from_principal(&subj_alt_name, allowed_client_domains)
            .context("Principal doesn't match realm pattern")?;
        info!("TLS authentication ok for user: {}", tls_username);
        username = Some(tls_username);
    };

    let mut conn = rustls::Stream::new(&mut tls_conn, &mut socket);

    // Send client version
    let server_version = tunnel::Version {
        version: VERSION.to_owned(),
    };
    send_server_msg_type!(&mut conn, server_version, Version).context("Error in send Version")?;

    /* Recv client version */
    let client_version: tunnel::Version =
        recv_client_msg_type!(&mut conn, Version).context("Error in send client version")?;

    info!("Client version {:?}", client_version);
    if client_version.version != VERSION {
        return Err(anyhow!(
            "Version mismatch server: {:?} client: {:?}",
            VERSION,
            client_version.version
        ));
    }

    if let Some(auth_type) = &config.auth_type {
        match auth_type {
            #[cfg(all(unix, feature = "kerberos"))]
            AuthType::Kerberos(realms) => {
                let krb_username = do_kerberos_client_auth(realms, &mut conn)?;
                info!("Kerberos authentication ok for user: {}", krb_username);
                if username.is_some() && username != Some(krb_username.to_owned()) {
                    return Err(send_server_err_event(
                        &mut conn,
                        anyhow!("Username mismatch between tls and kerberos"),
                    ));
                } else {
                    username = Some(krb_username);
                }
            }
            AuthType::Pam(pam_name) => {
                let final_user = do_pam_auth(&mut conn, pam_name)?;
                info!("Pam authentication ok for user: {}", final_user);
                username = Some(final_user);
            }
        }
    }

    let username = username.context("No username")?;
    info!("Authenticated user: {:?} from {:?}", username, addr);
    Ok((tls_conn, username, client_version))
}

/// Forward connection between peers
fn loop_fwd_conn(
    server: std::os::unix::net::UnixStream,
    client: std::net::TcpStream,
    mut tls_conn: ServerConnection,
) -> Result<()> {
    let mut input_buffer = vec![0u8; 1024 * 1024];
    let mut output_buffer = vec![0u8; 1024 * 1024];

    let mut server = UnixStream::from_std(server);
    let mut client = TcpStream::from_std(client);

    let mut poll = Poll::new().context("Error in poll")?;
    let mut events = Events::with_capacity(128);
    poll.registry()
        .register(&mut server, SERVER, Interest::READABLE)
        .context("Error in register server")?;
    poll.registry()
        .register(&mut client, CLIENT, Interest::READABLE)
        .context("Error in register client")?;

    let mut client = rustls::Stream::new(&mut tls_conn, &mut client);

    let mut stop = false;
    while !stop {
        poll.poll(&mut events, None).context("Error in poll")?;
        for event in events.iter() {
            match event.token() {
                CLIENT => {
                    let size = client
                        .read(&mut input_buffer)
                        .context("Error in client read")?;
                    trace!("forward to server {:?}", size);
                    if size == 0 {
                        debug!("Client closed connection");
                        stop = true;
                        break;
                    }
                    server
                        .write_all(&input_buffer[..size])
                        .context("Error in server write")?;
                }
                SERVER => {
                    let size = server
                        .read(&mut output_buffer)
                        .context("Error in server read")?;
                    trace!("forward to client {:?}", size);
                    if size == 0 {
                        debug!("Server closed connection");
                        stop = true;
                        break;
                    }
                    client
                        .write_all(&output_buffer[..size])
                        .context("Error in client write")?;
                }
                _ => unreachable!(),
            }
        }
    }
    Ok(())
}

/// Run callback and forward connection between client and son
pub fn connect_user(
    config: &Config,
    client: std::net::TcpStream,
    tls_conn: ServerConnection,
    username: &str,
    addr: &SocketAddr,
    client_version: tunnel::Version,
) -> Result<()> {
    // Create socket file
    let uuid = Uuid::new_v4();
    let socket_path = format!("/tmp/video_{uuid}");
    debug!("Bind unix socket {:?}", socket_path);

    let on_connect = &config.cmd_callback.on_connect;
    let args = replace_source(&on_connect.command_args, TOKEN_USERNAME, username);
    let args = replace_source(&args, TOKEN_CLIENT_ADDR, &addr.to_string());
    let args = replace_source(&args, TOKEN_UNIX_SOCKET_PATH, &socket_path);

    let listener = std::os::unix::net::UnixListener::bind(&socket_path)
        .context(format!("Error in UnixListener bind {socket_path:?}"))?;

    debug!("bin {} args {:?}", on_connect.command_bin, args);
    let status = process::Command::new(&on_connect.command_bin)
        .args(&args)
        .status()
        .context("Cannot exec connect callback")?;

    if !status.success() {
        return Err(anyhow!("Command execution failed"));
    }

    let (mut server, addr) = listener.accept().context("failed to accept connection")?;
    info!("Client {:?}", addr);

    // Forward client version to son
    send_client_msg_type!(&mut server, client_version, Version).context("Error in send Version")?;

    /* Recv client version */
    let server_version: tunnel::Version =
        recv_server_msg_type!(&mut server, Version).context("Error in send server version")?;

    info!("Server version {:?}", server_version);
    if server_version.version != VERSION {
        return Err(anyhow!(
            "Version mismatch server: {:?} client: {:?}",
            server_version.version,
            VERSION,
        ));
    }

    // Link client & proxy
    if let Err(err) = loop_fwd_conn(server, client, tls_conn) {
        error!("Connection error: {:?}", err);
    }

    info!("User disconnected: {:?}", username);
    remove_file(socket_path).context("Error in remove_file")?;

    Ok(())
}

/// Authenticate client and forward connection to son
/// Detach son from parent.
fn auth_and_connect(config: &Config, mut sock: std::net::TcpStream, addr: SocketAddr) {
    match unsafe { fork() } {
        Ok(ForkResult::Parent { .. }) => {
            // kill parent to detach son
            unsafe { libc::exit(0) };
        }
        Ok(ForkResult::Child) => {
            // Son continues
        }
        Err(_) => {
            error!("Fork failed");
            unsafe { libc::exit(1) };
        }
    }

    // Create a new SID for the child process
    if nix::unistd::setsid().is_err() {
        error!("Cannot create sid");
        unsafe { libc::exit(1) };
    }

    // Chdir to /
    if nix::unistd::chdir("/").is_err() {
        error!("Cannot set to /");
        unsafe { libc::exit(1) };
    }

    let (tls_conn, username, client_version) = match auth_client(config, &mut sock, &addr) {
        Ok((tls_conn, username, client_version)) => (tls_conn, username, client_version),
        Err(err) => {
            error!("Error in client auth {:?}", err);
            unsafe { libc::exit(1) };
        }
    };

    if let Err(err) = connect_user(config, sock, tls_conn, &username, &addr, client_version) {
        error!("Error for client {}: {:?}", addr, err);
    }
    unsafe { libc::exit(0) };
}

/// Accept and dispatch clients connections
fn serve_user(
    config: &Config,
    address: IpAddr,
    port: u16,
    connection_timeout: Option<u32>,
) -> Result<()> {
    info!("Server loop");
    let listener = std::net::TcpListener::bind(SocketAddr::new(address, port))
        .context(format!("Error in TcpListener bind {address} {port}"))?;

    let connection_timeout =
        connection_timeout.map(|timeout| std::time::Duration::from_secs(timeout as u64));

    let socket_ref = socket2::SockRef::from(&listener);
    set_tcp_timeout(socket_ref, connection_timeout).context("Cannot set keepalive")?;

    loop {
        let (sock, addr) = listener.accept().context("Failed to accept connection")?;

        info!("Client {:?}", addr);

        match unsafe { fork() } {
            Ok(ForkResult::Parent { child, .. }) => {
                // Force client sock drop
                drop(sock);
                waitpid(child, None).unwrap();
            }
            Ok(ForkResult::Child) => {
                // Force listener drop to free port
                drop(listener);
                auth_and_connect(config, sock, addr);
                break;
            }
            Err(_) => error!("Fork failed"),
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    env_logger::Builder::from_default_env().init();

    let about = format!(
        r#"Sanzu broker

Protocol version: {VERSION:?}
"#
    );

    let matches = Command::new("Surf server")
        .version(env!("CARGO_PKG_VERSION"))
        .about(about)
        .arg(
            Arg::new("config")
                .short('f')
                .long("config")
                .help("configuration file")
                .default_value(DEFAULT_CONFIG)
                .num_args(1),
        )
        .arg(
            Arg::new("listen")
                .short('l')
                .long("listen")
                .num_args(1)
                .default_value("127.0.0.1")
                .value_parser(clap::value_parser!(IpAddr))
                .help("Listen address"),
        )
        .arg(
            Arg::new("port")
                .short('p')
                .long("port")
                .num_args(1)
                .default_value("1122")
                .value_parser(clap::value_parser!(u16))
                .help("Bind port number"),
        )
        .arg(
            Arg::new("connection_timeout")
                .long("connection-timeout")
                .help("TCP timeout")
                .num_args(1)
                .value_parser(clap::value_parser!(u32)),
        )
        .get_matches();

    let address = *matches
        .get_one::<IpAddr>("listen")
        .context("Cannot parse listen address")?;

    let port = *matches
        .get_one::<u16>("port")
        .context("Cannot parse port")?;

    let connection_timeout = matches.get_one::<u32>("connection-timeout").copied();

    let config = read_config(
        matches
            .get_one::<String>("config")
            .context("Error in config path")?,
    )
    .context("Error in read_config")?;

    if let Err(err) = serve_user(&config, address, port, connection_timeout) {
        error!("Server error");
        err.chain().for_each(|cause| error!(" - due to {}", cause));
    }

    Ok(())
}
