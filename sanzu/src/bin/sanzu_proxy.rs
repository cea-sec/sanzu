#[macro_use]
extern crate log;

use clap::{Arg, Command};
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
        .about(about.as_str())
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
                .takes_value(true),
        )
        .arg(
            Arg::new("vsock")
                .short('v')
                .long("vsock")
                .takes_value(false)
                .help("Use vsock"),
        )
        .arg(
            Arg::new("listen")
                .short('l')
                .long("listen")
                .takes_value(true)
                .default_value("127.0.0.1")
                .help("Listen address"),
        )
        .arg(
            Arg::new("listen_port")
                .short('p')
                .long("port")
                .takes_value(true)
                .help("Bind port number"),
        )
        .arg(
            Arg::new("connect_unix_socket")
                .short('u')
                .long("unix_socket")
                .takes_value(true)
                .help("Path of the unix socket to connect to instead of listening for the client"),
        )
        .arg(
            Arg::new("encoder")
                .short('e')
                .long("encoder")
                .takes_value(true)
                .help("Encoder name (libx264, h264_nvenc, ...)"),
        )
        .arg(
            Arg::new("audio")
                .help("Allow audio forwarding")
                .short('a')
                .long("audio")
                .takes_value(false),
        )
        .arg(
            Arg::new("import_video_shm")
                .short('i')
                .long("import_video_shm")
                .takes_value(true)
                .help(
                    "Input video from shared memory\n\
                     Example: if the video server runs in a vm,\n\
                     the video buffer is exfiltrer using guest/host shared memory instead of\n\
                     tcp or vsock",
                ),
        )
        .arg(
            Arg::new("shm_is_xwd")
                .help("Input from shared memory is in xwd format")
                .short('x')
                .long("shm_is_xwd")
                .takes_value(false),
        )
        .get_matches();

    let server_addr = matches
        .value_of("server_ip")
        .expect("Cannot parse server ip address");

    let listen_address = matches
        .value_of("listen")
        .unwrap()
        .parse::<IpAddr>()
        .expect("Cannot parse listen address");

    let server_port = matches.value_of("server_port").unwrap();

    let listen_port = matches
        .value_of("listen_port")
        .map(|x| x.parse::<u16>().expect("Cannot parse port"));

    let unix_socket = matches.value_of("connect_unix_socket");

    let audio = matches.is_present("audio");
    let encoder_name = matches.value_of("encoder").unwrap_or("libx264");
    let conf = read_server_config(matches.value_of("config").unwrap()).unwrap();
    let vsock = matches.is_present("vsock");
    let import_video_shm = matches.value_of("import_video_shm");
    let shm_is_xwd = matches.is_present("shm_is_xwd");

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
    };

    if let Err(err) = proxy::run(&conf, &arguments) {
        error!("Proxy error");
        err.chain().for_each(|cause| error!(" - due to {}", cause));
    }
}
