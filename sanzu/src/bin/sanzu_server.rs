use anyhow::{Context, Result};

#[macro_use]
extern crate log;

use clap::{Arg, Command};

use sanzu::{config::read_server_config, server, utils::ArgumentsSrv};

const DEFAULT_CONFIG: &str = "/etc/sanzu.toml";

fn main() -> Result<()> {
    env_logger::Builder::from_default_env()
        .format_timestamp_nanos()
        .init();

    let matches = Command::new("Sanzu server")
        .version("0.1.0")
        .about("Stream server x11 to h264/?")
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
            Arg::new("unixsock")
                .short('u')
                .long("unixsock")
                .takes_value(false)
                .help("Use unixsocket"),
        )
        .arg(
            Arg::new("connect-unixsock")
                .short('c')
                .long("connect-unixsock")
                .takes_value(false)
                .help("connect to unixsocket instead of listening"),
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
            Arg::new("stdio")
                .long("stdio")
                .takes_value(false)
                .help("Uses STDIO instead of listining on a TCP port"),
        )
        .arg(
            Arg::new("port")
                .short('p')
                .long("port")
                .takes_value(true)
                .default_value("1122")
                .help("Bind port number"),
        )
        .arg(
            Arg::new("encoder")
                .short('e')
                .long("encoder")
                .takes_value(true)
                .help("Encoder name (libx264, h264_nvenc, ...)"),
        )
        .arg(
            Arg::new("seamless")
                .help("Seamless mode")
                .short('s')
                .long("seamless")
                .takes_value(false),
        )
        .arg(
            Arg::new("keep_server_resolution")
                .help("Server will keep it's resolution")
                .short('x')
                .long("keep_server_resolution")
                .takes_value(false),
        )
        .arg(
            Arg::new("audio")
                .help("Allow audio forwarding")
                .short('a')
                .long("audio")
                .takes_value(false),
        )
        .arg(
            Arg::new("raw_sound")
                .short('r')
                .long("raw_sound")
                .takes_value(false)
                .help("Transmit Raw sound"),
        )
        .arg(
            Arg::new("export_video_pci")
                .short('i')
                .long("export_video_pci")
                .takes_value(false)
                .help(
                    "Export video to a pci shared memory\n\
                     Example: if the video server runs in a vm,\n\
                     the video buffer is exfiltrer using guest/host shared memory instead of\n\
                     tcp or vsock",
                ),
        )
        .arg(
            Arg::new("use_extern_img_source")
                .short('k')
                .long("use_extern_img_source")
                .takes_value(true)
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
                .takes_value(false),
        )
        .get_matches();

    let address = matches.value_of("listen").unwrap();

    let port = matches.value_of("port").unwrap();

    let seamless = matches.is_present("seamless");
    let keep_server_resolution = matches.is_present("keep_server_resolution");
    let audio = matches.is_present("audio");
    let encoder_name = matches.value_of("encoder").unwrap_or("libx264");
    let raw_sound = matches.is_present("raw_sound");
    let conf = read_server_config(matches.value_of("config").unwrap())
        .context("Cannot read configuration file")?;
    let vsock = matches.is_present("vsock");
    let stdio = matches.is_present("stdio");
    let unixsock = matches.is_present("unixsock");
    let connect_unixsock = matches.is_present("connect-unixsock");
    let export_video_pci = matches.is_present("export_video_pci");
    let restrict_clipboard = matches.is_present("restrict-clipboard");
    let extern_img_source = matches.value_of("use_extern_img_source");
    let avoid_img_extraction = matches.is_present("avoid_img_extraction");

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
    };

    if let Err(err) = server::run(&conf, &arguments) {
        error!("Server error");
        err.chain().for_each(|cause| error!(" - due to {}", cause));
    }
    Ok(())
}
