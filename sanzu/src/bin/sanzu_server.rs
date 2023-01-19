use anyhow::{Context, Result};

#[macro_use]
extern crate log;

use clap::{Arg, ArgAction, Command};

use sanzu::{config::read_server_config, server, utils::ArgumentsSrv};

use sanzu_common::proto::VERSION;

const DEFAULT_CONFIG: &str = "/etc/sanzu.toml";

fn main() -> Result<()> {
    env_logger::Builder::from_default_env()
        .format_timestamp_nanos()
        .init();

    let about = format!(
        r#"Sanzu server: desktop video streaming

Protocol version: {:?}
"#,
        VERSION
    );

    let matches = Command::new("Sanzu server")
        .version("0.1.0")
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
            Arg::new("vsock")
                .short('v')
                .long("vsock")
                .action(ArgAction::SetTrue)
                .help("Use vsock"),
        )
        .arg(
            Arg::new("unixsock")
                .short('u')
                .long("unixsock")
                .action(ArgAction::SetTrue)
                .help("Use unixsocket"),
        )
        .arg(
            Arg::new("connect-unixsock")
                .short('c')
                .long("connect-unixsock")
                .action(ArgAction::SetTrue)
                .help("connect to unixsocket instead of listening"),
        )
        .arg(
            Arg::new("listen")
                .short('l')
                .long("listen")
                .num_args(1)
                .default_value("127.0.0.1")
                .help("Listen address"),
        )
        .arg(
            Arg::new("stdio")
                .short('m')
                .long("stdio")
                .action(ArgAction::SetTrue)
                .help("Uses STDIO instead of listining on a TCP port"),
        )
        .arg(
            Arg::new("port")
                .short('p')
                .long("port")
                .num_args(1)
                .default_value("1122")
                .help("Bind port number"),
        )
        .arg(
            Arg::new("encoder")
                .short('e')
                .long("encoder")
                .num_args(1)
                .help("Encoder name (libx264, h264_nvenc, ...)"),
        )
        .arg(
            Arg::new("seamless")
                .help("Seamless mode")
                .short('s')
                .long("seamless")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("keep_server_resolution")
                .help("Server will keep it's resolution")
                .short('x')
                .long("keep_server_resolution")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("audio")
                .help("Allow audio forwarding")
                .short('a')
                .long("audio")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("raw_sound")
                .short('r')
                .long("raw_sound")
                .action(ArgAction::SetTrue)
                .help("Transmit Raw sound"),
        )
        .arg(
            Arg::new("export_video_pci")
                .short('i')
                .long("export_video_pci")
                .action(ArgAction::SetTrue)
                .help(
                    "Export video to a pci shared memory\n\
                     Example: if the video server runs in a vm,\n\
                     the video buffer is exfiltrated using guest/host shared memory instead of\n\
                     tcp or vsock",
                ),
        )
        .arg(
            Arg::new("use_extern_img_source")
                .short('k')
                .long("use_extern_img_source")
                .num_args(1)
                .help(
                    "Do not extract image from x11\n\
                     Example: if you use Xvfb you can use Xvfb backbuffer file\n\
                     instead of extracting images from x11 server",
                ),
        )
        .arg(
            Arg::new("avoid_img_extraction")
                .short('j')
                .long("avoid_img_extraction")
                .action(ArgAction::SetTrue)
                .help(
                    "The video server considers that image extraction will be done by\n\
                     another process. Empty images will we sent to client.\n\
                     Use case: combined with a proxy encoder from a virtual machine",
                ),
        )
        .arg(
            Arg::new("restrict-clipboard")
                .help("Don't send clipboard to client")
                .short('q')
                .long("restrict-clipboard")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("rdonly")
                .help("Read only server")
                .short('o')
                .long("rdonly")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("loop")
                .help("Endless server clients")
                .short('d')
                .long("loop")
                .action(ArgAction::SetTrue),
        )
        .get_matches();

    let address = matches.get_one::<String>("listen").unwrap();

    let port = matches.get_one::<String>("port").unwrap();

    let seamless = matches.get_flag("seamless");
    let keep_server_resolution = matches.get_flag("keep_server_resolution");
    let audio = matches.get_flag("audio");
    let encoder_name = matches
        .get_one::<String>("encoder")
        .unwrap_or(&"libx264".to_string())
        .to_owned();

    let raw_sound = matches.get_flag("raw_sound");
    let conf = read_server_config(matches.get_one::<String>("config").unwrap())
        .context("Cannot read configuration file")?;
    let vsock = matches.get_flag("vsock");
    let stdio = matches.get_flag("stdio");
    let unixsock = matches.get_flag("unixsock");
    let connect_unixsock = matches.get_flag("connect-unixsock");
    let export_video_pci = matches.get_flag("export_video_pci");
    let restrict_clipboard = matches.get_flag("restrict-clipboard");
    let extern_img_source = matches.get_one::<String>("use_extern_img_source").cloned();
    let avoid_img_extraction = matches.get_flag("avoid_img_extraction");
    let rdonly = matches.get_flag("rdonly");
    let endless_loop = matches.get_flag("loop");

    let arguments = ArgumentsSrv {
        vsock,
        stdio,
        unixsock,
        connect_unixsock,
        address,
        port,
        encoder_name,
        seamless,
        keep_server_resolution,
        audio,
        raw_sound,
        export_video_pci,
        restrict_clipboard,
        extern_img_source,
        avoid_img_extraction,
        rdonly,
        endless_loop,
    };

    if let Err(err) = server::run(&conf, &arguments) {
        error!("Server error");
        err.chain().for_each(|cause| error!(" - due to {}", cause));
    }
    Ok(())
}
