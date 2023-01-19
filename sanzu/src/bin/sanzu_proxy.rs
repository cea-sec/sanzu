#[macro_use]
extern crate log;

use clap::{Arg, ArgAction, Command};
use sanzu::{config::read_server_config, proxy, utils::ArgumentsProxy};
use sanzu_common::proto::VERSION;
use std::net::IpAddr;

const DEFAULT_CONFIG: &str = "/etc/sanzu.toml";

fn main() {
    env_logger::Builder::from_default_env().init();

    let about = format!(
        r#"Sanzu proxy: desktop video streaming

Protocol version: {:?}
"#,
        VERSION
    );

    let matches = Command::new("Sanzu proxy")
        .version("0.1.0")
        .about(about)
        .arg(
            Arg::new("server_ip")
                .help("Sets the server IP (Ex: 127.0.0.1)")
                .required(true)
                .index(1),
        )
        .arg(
            Arg::new("server_port")
                .help("Sets the server port (Ex: 122)")
                .required(true)
                .index(2),
        )
        .arg(
            Arg::new("config")
                .short('f')
                .long("config")
                .help("configuration file")
                .default_value(DEFAULT_CONFIG)
                .num_args(1),
        )
        .arg(
            Arg::new("vsock")
                .short('v')
                .long("vsock")
                .num_args(0)
                .help("Use vsock"),
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
            Arg::new("listen_port")
                .short('p')
                .long("port")
                .num_args(1)
                .value_parser(clap::value_parser!(u16))
                .help("Bind port number"),
        )
        .arg(
            Arg::new("connect_unix_socket")
                .short('u')
                .long("unix_socket")
                .num_args(1)
                .help("Path of the unix socket to connect to instead of listening for the client"),
        )
        .arg(
            Arg::new("encoder")
                .short('e')
                .long("encoder")
                .num_args(1)
                .help("Encoder name (libx264, h264_nvenc, ...)"),
        )
        .arg(
            Arg::new("audio")
                .help("Allow audio forwarding")
                .short('a')
                .long("audio")
                .num_args(0),
        )
        .arg(
            Arg::new("import_video_shm")
                .short('i')
                .long("import_video_shm")
                .num_args(1)
                .help(
                    "Input video from shared memory\n\
                     Example: if the video server runs in a vm,\n\
                     the video buffer is exfiltrated using guest/host shared memory instead of\n\
                     tcp or vsock",
                ),
        )
        .arg(
            Arg::new("shm_is_xwd")
                .help("Input from shared memory is in xwd format")
                .short('x')
                .long("shm_is_xwd")
                .num_args(0),
        )
        .arg(
            Arg::new("loop")
                .help("Endless server clients")
                .short('d')
                .long("loop")
                .action(ArgAction::SetTrue),
        )
        .get_matches();

    let server_addr = matches
        .get_one::<String>("server_ip")
        .expect("Cannot parse server ip address");

    let listen_address = *matches.get_one::<IpAddr>("listen").unwrap();

    let server_port = matches.get_one::<String>("server_port").unwrap();

    let listen_port = matches.get_one::<u16>("listen_port").cloned();

    let unix_socket = matches.get_one::<String>("connect_unix_socket").cloned();

    let audio = matches.get_flag("audio");
    let encoder_name = matches
        .get_one::<String>("encoder")
        .unwrap_or(&"libx264".to_string())
        .to_owned();
    let conf = read_server_config(matches.get_one::<String>("config").unwrap()).unwrap();
    let vsock = matches.get_flag("vsock");
    let import_video_shm = matches.get_one::<String>("import_video_shm").cloned();
    let shm_is_xwd = matches.get_flag("shm_is_xwd");
    let endless_loop = matches.get_flag("loop");

    let arguments = ArgumentsProxy {
        vsock,
        server_addr,
        server_port,
        listen_address,
        listen_port,
        unix_socket,
        encoder_name,
        audio,
        video_shared_mem: import_video_shm,
        shm_is_xwd,
        endless_loop,
    };

    if let Err(err) = proxy::run(&conf, &arguments) {
        error!("Proxy error");
        err.chain().for_each(|cause| error!(" - due to {}", cause));
    }
}
