#![windows_subsystem = "windows"]
#[macro_use]
extern crate log;

use clap::{builder::PossibleValue, Arg, ArgAction, Command};

use sanzu::{
    client,
    config::{read_client_config, ConfigClient},
    utils::{ArgumentsClient, ClipboardConfig},
};

use sanzu_common::proto::VERSION;

use std::collections::HashMap;

#[cfg(windows)]
use winapi::um::wincon;

fn main() {
    env_logger::Builder::from_default_env()
        .format_timestamp_nanos()
        .init();

    let about = format!(
        r#"Sanzu client: desktop video streaming

Protocol version: {VERSION:?}

To change log level:
RUST_LOG=debug
RUST_LOG=info
"#
    );

    #[cfg(windows)]
    {
        unsafe {
            wincon::AttachConsole(wincon::ATTACH_PARENT_PROCESS);
        }
    }

    let app = Command::new("Sanzu client")
        .version("0.1.0")
        .about(about)
        .arg(
            Arg::new("ip")
                .help("Sets the server IP (Ex: 127.0.0.1)")
                .required(true)
                .index(1),
        )
        .arg(
            Arg::new("port")
                .help("Sets the server port (Ex: 1122)")
                .required(true)
                .value_parser(clap::value_parser!(u16))
                .index(2),
        )
        .arg(
            Arg::new("config")
                .short('f')
                .long("config")
                .help("configuration file")
                .num_args(1),
        )
        .arg(
            Arg::new("audio")
                .help("Forward audio")
                .short('a')
                .long("audio")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("tls_ca")
                .help("Enable tls encryption / server authentication")
                .short('t')
                .long("tls_ca")
                .num_args(1),
        )
        .arg(
            Arg::new("tls_server_name")
                .help("Server name for tls tunnel")
                .short('n')
                .long("tls_server_name")
                .num_args(1),
        )
        .arg(
            Arg::new("client_cert")
                .help("Use client cert authentication")
                .short('c')
                .long("client_cert")
                .num_args(1),
        )
        .arg(
            Arg::new("client_key")
                .help("Client cert key")
                .short('x')
                .long("client_key")
                .num_args(1),
        )
        .arg(
            Arg::new("audio_buffer_ms")
                .help("Audio buffer ms (default: 150ms)")
                .short('b')
                .long("audio_buffer_ms")
                .value_parser(clap::value_parser!(u32))
                .num_args(1),
        )
        .arg(
            Arg::new("audio_sample_rate")
                .help("Audio sample rate")
                .short('s')
                .long("audio_sample_rate")
                .value_parser(clap::value_parser!(u32))
                .num_args(1),
        )
        .arg(
            Arg::new("login")
                .help("Use login/password to authenticate")
                .short('l')
                .long("login")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("clipboard")
                .help(r#"Control clipboard behavior:
 - allow: send clipboard to server on local clipboard modification
 - deny: never send local clipboard to server
 - trig: send local clipboard to server on hitting special shortcut
"#)
                .short('q')
                .long("clipboard")
                .num_args(1)
                .default_missing_value("allow")
                .value_parser([
                    PossibleValue::new("allow"),
                    PossibleValue::new("deny"),
                    PossibleValue::new("trig"),
                ]),
        )
        .arg(
            Arg::new("window-mode")
                .help("Client will be in window mode instead of fullscreen")
                .short('w')
                .long("window-mode")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("decoder")
                .short('d')
                .long("decoder")
                .num_args(1)
                .help("Force decoder name (libx264, h264_qsv, ...) (must be compatible with selected encoder)"),
        )
        .arg(
            Arg::new("allow-print")
                .short('j')
                .long("allow-print")
                .num_args(1)
                .help(
                    r#""Allow print order from serveur to local printing service.
The argument is the local firectory base which contains files to print
Ex: -j c:\user\dupond\printdir\
"#)
        )
        .arg(
            Arg::new("sync_key_locks")
                .help("Synchronize caps/num/scroll lock")
                .short('g')
                .long("sync_key_locks")
                .action(ArgAction::SetTrue),
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
                .short('y')
                .long("shm_is_xwd")
                .num_args(0),
        )
        .arg(
            Arg::new("proxycommand")
                .short('p')
                .long("proxycommand")
                .num_args(1)
                .help("Command to execute to establish connection"),
        );

    #[cfg(feature = "kerberos")]
    let app = app.arg(
        Arg::new("server_cname")
            .help("Enable kerberos using server cname")
            .short('k')
            .long("server_cname")
            .num_args(1),
    );

    let matches = app.get_matches();

    let server_ip = matches
        .get_one::<String>("ip")
        .expect("IP server is mandatory");

    let server_port = *matches.get_one::<u16>("port").unwrap_or(&1122);

    let audio_buffer_ms = *matches.get_one::<u32>("audio_buffer_ms").unwrap_or(&150);

    let audio_sample_rate = matches.get_one::<u32>("audio_sample_rate").cloned();

    let audio = matches.get_flag("audio");
    let server_cname = matches.get_one::<String>("server_cname").cloned();
    let tls_ca = matches.get_one::<String>("tls_ca").cloned();
    let client_cert = matches.get_one::<String>("client_cert").cloned();
    let client_key = matches.get_one::<String>("client_key").cloned();
    let tls_server_name = matches.get_one::<String>("tls_server_name").cloned();
    let login = matches.get_flag("login");

    let clipboard_config = match matches
        .get_one::<String>("clipboard")
        .unwrap_or(&"allow".to_string())
        .as_str()
    {
        "allow" => ClipboardConfig::Allow,
        "deny" => ClipboardConfig::Deny,
        "trig" => ClipboardConfig::Trig,
        _ => {
            panic!("Unknown clipboard configuration");
        }
    };

    let window_mode = matches.get_flag("window-mode");
    let decoder_name = matches.get_one::<String>("decoder").cloned();
    let printdir = matches.get_one::<String>("allow-print").cloned();
    let client_config = match matches.get_one::<String>("config") {
        Some(config_path) => {
            read_client_config(config_path).expect("Cannot read configuration file")
        }
        None => ConfigClient {
            ffmpeg: HashMap::new(),
        },
    };
    let proxycommand = matches.get_one::<String>("proxycommand").cloned();
    let sync_key_locks = matches.get_flag("sync_key_locks");
    let import_video_shm = matches.get_one::<String>("import_video_shm").cloned();
    let shm_is_xwd = matches.get_flag("shm_is_xwd");

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
        clipboard_config,
        window_mode,
        decoder_name,
        printdir,
        proxycommand,
        sync_key_locks,
        video_shared_mem: import_video_shm,
        shm_is_xwd,
    };

    if let Err(err) = client::run(
        &client_config,
        &arguments,
        client::StdioClientInterface::default(),
    ) {
        error!("Client error");
        err.chain().for_each(|cause| error!(" - due to {}", cause));
    }
}
