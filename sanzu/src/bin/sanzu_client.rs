#[macro_use]
extern crate log;

use clap::{Arg, Command};

use sanzu::{
    client,
    config::{read_client_config, ConfigClient},
    utils::ArgumentsClient,
};
use std::collections::HashMap;

fn main() {
    env_logger::Builder::from_default_env()
        .format_timestamp_nanos()
        .init();

    let app = Command::new("Sanzu client")
        .version("0.1.0")
        .about(
            r#"Stream client x11 from h264/?

To change log level:
RUST_LOG=debug
RUST_LOG=info
"#,
        )
        .arg(
            Arg::new("ip")
                .help("Sets the server IP (Ex: 127.0.0.1)")
                .required(true)
                .index(1),
        )
        .arg(
            Arg::new("port")
                .help("Sets the server port (Ex: 122)")
                .required(true)
                .index(2),
        )
        .arg(
            Arg::new("config")
                .short('f')
                .long("config")
                .help("configuration file")
                .takes_value(true),
        )
        .arg(
            Arg::new("audio")
                .help("Forward audio")
                .short('a')
                .long("audio")
                .takes_value(false),
        )
        .arg(
            Arg::new("tls_ca")
                .help("Enable tls encryption / server authentication")
                .short('t')
                .long("tls_ca")
                .takes_value(true),
        )
        .arg(
            Arg::new("tls_server_name")
                .help("Server name for tls tunnel")
                .short('n')
                .long("tls_server_name")
                .takes_value(true),
        )
        .arg(
            Arg::new("client_cert")
                .help("Use client cert authentication")
                .short('c')
                .long("client_cert")
                .takes_value(true),
        )
        .arg(
            Arg::new("client_key")
                .help("Client cert key")
                .short('x')
                .long("client_key")
                .takes_value(true),
        )
        .arg(
            Arg::new("audio_buffer_ms")
                .help("Audio buffer ms (default: 150ms)")
                .short('b')
                .long("audio_buffer_ms")
                .takes_value(true),
        )
        .arg(
            Arg::new("audio_sample_rate")
                .help("Audio sample rate")
                .short('s')
                .long("audio_sample_rate")
                .takes_value(true),
        )
        .arg(
            Arg::new("login")
                .help("Use login/password to authenticate")
                .short('l')
                .long("login")
                .takes_value(false),
        )
        .arg(
            Arg::new("restrict-clipboard")
                .help("Don't send clipboard to server")
                .short('q')
                .long("restrict-clipboard")
                .takes_value(false),
        )
        .arg(
            Arg::new("window-mode")
                .help("Client will be in window mode instead of fullscreen")
                .short('w')
                .long("window-mode")
                .takes_value(false),
        )
        .arg(
            Arg::new("decoder")
                .short('d')
                .long("decoder")
                .takes_value(true)
                .help("Force decoder name (libx264, h264_qsv, ...) (must be compatible with selected encoder)"),
        )
        .arg(
            Arg::new("allow-print")
                .short('j')
                .long("allow-print")
                .takes_value(true)
                .help(
                    r#""Allow print order from serveur to local printing service.
The argument is the local firectory base which contains files to print
Ex: -j c:\user\dupond\printdir\
"#)
        )
        .arg(
            Arg::new("proxycommand")
                .short('p')
                .long("proxycommand")
                .takes_value(true)
                .help("Command to execute to establish connection"),
        );

    #[cfg(feature = "kerberos")]
    let app = app.arg(
        Arg::new("server_cname")
            .help("Enable kerberos using server cname")
            .short('k')
            .long("server_cname")
            .takes_value(true),
    );

    let matches = app.get_matches();

    let server_ip = matches.value_of("ip").expect("IP server is mandatory");

    let server_port: u16 = matches
        .value_of("port")
        .unwrap_or("1122")
        .parse::<u16>()
        .expect("Cannot parse port");

    let audio_buffer_ms: u32 = matches
        .value_of("audio_buffer_ms")
        .unwrap_or("150")
        .parse::<u32>()
        .expect("Cannot parse audio_buffer_ms");

    let audio_sample_rate = matches
        .value_of("audio_sample_rate")
        .map(|audio_sample_rate| {
            audio_sample_rate
                .parse::<u32>()
                .expect("Cannot parse audio_sample_rate")
        });

    let audio = matches.is_present("audio");
    let server_cname = matches.value_of("server_cname");
    let tls_ca = matches.value_of("tls_ca");
    let client_cert = matches.value_of("client_cert");
    let client_key = matches.value_of("client_key");
    let tls_server_name = matches.value_of("tls_server_name");
    let login = matches.is_present("login");
    let restrict_clipboard = matches.is_present("restrict-clipboard");
    let window_mode = matches.is_present("window-mode");
    let decoder_name = matches.value_of("decoder");
    let printdir = matches.value_of("allow-print");
    let client_config = match matches.value_of("config") {
        Some(config_path) => {
            read_client_config(config_path).expect("Cannot read configuration file")
        }
        None => ConfigClient {
            ffmpeg: HashMap::new(),
        },
    };
    let proxycommand = matches.value_of("proxycommand");

    let arguments = ArgumentsClient {
        address: server_ip,
        port: server_port,
        audio,
        audio_sample_rate,
        audio_buffer_ms,
        server_cname,
        tls_ca,
        tls_server_name,
        client_cert,
        client_key,
        login,
        restrict_clipboard,
        window_mode,
        decoder_name,
        printdir,
        proxycommand,
    };

    if let Err(err) = client::run(&client_config, &arguments) {
        error!("Client error");
        err.chain().for_each(|cause| error!(" - due to {}", cause));
    }
}
