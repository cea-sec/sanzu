use anyhow::Result;
use byteorder::{BigEndian, ByteOrder};
use std::net::IpAddr;

use clap::{CommandFactory, Parser};
use twelf::config;

pub enum ServerEvent {
    ResolutionChange(u32, u32),
}

#[derive(Debug, Clone, Copy)]
pub enum ClipboardSelection {
    Clipboard,
    Primary,
}

#[derive(Debug, Clone, Copy)]
pub enum ClipboardConfig {
    Allow,
    Deny,
    Trig,
}

#[derive(Parser, Debug)]
#[clap(author, version, about)]
pub struct ServerArgs {
    /// Config file
    #[clap(
        short,
        long = "config_path",
        help = r"Path of toml file storing *arguments* configuration.
Sanzu arguments can be set regarding this priority:
- this configuration file
- environment variable
- command line"
    )]
    pub config_path: Option<std::path::PathBuf>,

    /// Rest of arguments
    #[clap(flatten)]
    pub server_config: ServerArgsConfig,
}

#[config]
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct ServerArgsConfig {
    #[clap(
        long,
        short = 'f',
        default_value = "/etc/sanzu.toml",
        help = "Sanzu server configuration file (video compression configuration, ...)"
    )]
    pub config: String,
    #[clap(
        long,
        default_value_t = false,
        help = "Use vsock for communication layer"
    )]
    pub vsock: bool,
    #[clap(
        long,
        short = 'm',
        default_value_t = false,
        help = "Use STDIO for communication layer"
    )]
    pub stdio: bool,
    #[clap(
        long,
        short = 'u',
        default_value_t = false,
        help = "Use unix socket for communication layer"
    )]
    pub unixsock: bool,
    #[clap(
        long,
        short = 'c',
        default_value_t = false,
        help = "Connect to unixsocket instead of listening"
    )]
    pub connect_unixsock: bool,
    #[clap(
        long,
        short = 'l',
        default_value = "127.0.0.1",
        help = "Listen address. Subnet or unix path"
    )]
    pub address: String,
    #[clap(long, short = 'p', default_value = "1122", help = "Bind port number")]
    pub port: String,
    #[clap(
        long,
        short = 'e',
        default_value = "libx264",
        help = "Encoder name. Ex: libx264, h264_qsv, hevc_nvenc"
    )]
    pub encoder: String,
    #[clap(
        long,
        short = 's',
        default_value_t = false,
        help = "Seamless mode. Integrate remote windows in local environement"
    )]
    pub seamless: bool,
    #[clap(
        long,
        short = 'x',
        default_value_t = false,
        help = "Server will keep it's resolution"
    )]
    pub keep_server_resolution: bool,
    #[clap(
        long,
        short = 'a',
        default_value_t = false,
        help = "Allow audio forwarding from server to client"
    )]
    pub audio: bool,
    #[clap(
        long,
        short = 'r',
        default_value_t = false,
        help = "Transmit Raw sound (not encoded)"
    )]
    pub raw_sound: bool,
    #[clap(
        long,
        default_value_t = false,
        help = r"Export video to a pci shared memory
Example: if the video server runs in a vm,
the video buffer is exfiltrated using guest/host shared memory instead of
tcp or vsock"
    )]
    pub export_video_pci: bool,
    #[clap(
        long,
        short = 'q',
        default_value_t = false,
        help = "Disallow sending clipboard from server to client"
    )]
    pub restrict_clipboard: bool,
    #[clap(
        long,
        short = 'k',
        help = "Use video source from file instead of video server api"
    )]
    pub extern_img_source: Option<String>,
    #[clap(
        long,
        short = 'j',
        default_value_t = false,
        help = "Skip video processing (use with sanzy_proxy for example)"
    )]
    pub avoid_img_extraction: bool,
    #[clap(
        long,
        short = 'o',
        default_value_t = false,
        help = "Simply stream video: client cannot interact"
    )]
    pub rdonly: bool,
    #[clap(
        long,
        short = 'd',
        default_value_t = false,
        help = "Loop if client disconnect instead of quitting"
    )]
    pub keep_listening: bool,
    #[clap(long, help = "Displays protocol version")]
    pub proto: bool,
    #[clap(short='v', long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

#[derive(Parser, Debug)]
#[clap(author, version, about)]
pub struct ClientArgs {
    /// Config file
    #[clap(
        short,
        long = "config_path",
        help = r"Path of toml file storing *arguments* configuration.
Sanzu arguments can be set regarding this priority:
- this configuration file
- environment variable
- command line"
    )]
    pub config_path: Option<std::path::PathBuf>,

    /// Rest of arguments
    #[clap(flatten)]
    pub client_config: ClientArgsConfig,
}

#[config]
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct ClientArgsConfig {
    #[clap(help = "Sanzu server address")]
    pub server_addr: String,
    #[clap(help = "Sanzu server port")]
    pub server_port: u16,
    #[clap(
        long,
        short = 'f',
        help = "Sanzu client configuration file (video decompression configuration, ...)"
    )]
    pub config: Option<String>,
    #[cfg(unix)]
    #[clap(
        long,
        default_value_t = false,
        help = "Use vsock for communication layer"
    )]
    pub vsock: bool,
    #[clap(
        long,
        short = 'a',
        default_value_t = false,
        help = "Allow audio forwarding from server to client"
    )]
    pub audio: bool,
    #[clap(long, short = 's', help = "Audio sample rate")]
    pub audio_sample_rate: Option<u32>,
    #[clap(
        long,
        short = 'b',
        default_value_t = 150,
        help = "Audio buffer length (default 150ms)"
    )]
    pub audio_buffer_ms: u32,
    #[cfg(feature = "kerberos")]
    #[clap(long, short = 'k', help = "Enable kerberos using server cname")]
    pub server_cname: Option<String>,
    #[clap(
        long,
        short = 't',
        help = "Enable tls encryption / server authentication"
    )]
    pub tls_ca: Option<String>,
    #[clap(long, short = 'n', help = "Server name for tls tunnel")]
    pub tls_server_name: Option<String>,
    #[clap(long, short = 'c', help = "Use client cert authentication")]
    pub client_cert: Option<String>,
    #[clap(
        long,
        short = 'x',
        help = "Client private key used in tls authentication"
    )]
    pub client_key: Option<String>,
    #[clap(long, short = 'l', help = "Use login/password to authenticate")]
    pub login: bool,
    #[clap(
        long,
        short = 'q',
        default_value = "allow",
        help = r#"Control clipboard behavior:
 - allow: send clipboard to server on local clipboard modification
 - deny: never send local clipboard to server
 - trig: send local clipboard to server on hitting special shortcut
"#
    )]
    pub clipboard: String,
    #[clap(
        long,
        short = 'w',
        help = "Client will be in window mode instead of fullscreen"
    )]
    pub window_mode: bool,
    #[clap(
        long,
        short = 'd',
        help = "Force decoder name (libx264, h264_qsv, ...) (must be compatible with selected encoder)"
    )]
    pub decoder: Option<String>,
    #[clap(
        long,
        short = 'j',
        help = r#""Allow print order from serveur to local printing service.
The argument is the local firectory base which contains files to print
Ex: -j c:\user\dupond\printdir\
"#
    )]
    pub allow_print: Option<String>,
    #[clap(long, short = 'p', help = "Command to execute to establish connection")]
    pub proxycommand: Option<String>,
    #[clap(long, short = 'g', help = "Synchronize caps/num/scroll lock")]
    pub sync_key_locks: bool,
    #[clap(
        long,
        short = 'k',
        help = r"Input video from shared memory
Example: if the video server runs in a vm,
the video buffer is exfiltrated using guest/host shared memory instead of
tcp or vsock"
    )]
    pub extern_img_source: Option<String>,
    #[clap(long, short = 'y', help = "Video source is xwd formated")]
    pub source_is_xwd: bool,
    #[clap(
        long,
        default_value = "Sanzu",
        help = "Window's title (default: 'Sanzu client')"
    )]
    pub title: String,
    #[clap(
        long,
        short = 'u',
        help = r"Grab and keep keyboard on focus.
This allows (linux) sending special keys like alt-tab
without being interpreted by the local window manager"
    )]
    pub grab_keyboard: bool,
    #[clap(long, help = "Displays protocol version")]
    pub proto: bool,
    #[clap(short='v', long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

#[derive(Parser, Debug)]
#[clap(author, version, about)]
pub struct ProxyArgs {
    /// Config file
    #[clap(
        short,
        long = "config_path",
        help = r"Path of toml file storing *arguments* configuration.
Sanzu arguments can be set regarding this priority:
- this configuration file
- environment variable
- command line"
    )]
    pub config_path: Option<std::path::PathBuf>,

    /// Rest of arguments
    #[clap(flatten)]
    pub proxy_config: ProxyArgsConfig,
}

#[config]
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct ProxyArgsConfig {
    #[clap(
        long,
        short = 'f',
        default_value = "/etc/sanzu.toml",
        help = "Sanzu server configuration file (video compression configuration, ...)"
    )]
    pub config: String,
    #[clap(
        long,
        default_value_t = false,
        help = "Use vsock for communication layer"
    )]
    pub vsock: bool,
    #[clap(help = "Sanzu server address")]
    pub server_addr: String,
    #[clap(help = "Sanzu server port")]
    pub server_port: String,
    #[clap(
        long,
        short = 'l',
        default_value = "127.0.0.1",
        help = "Listen address. Subnet or unix path"
    )]
    pub listen_address: IpAddr,
    #[clap(long, short = 'p', help = "Bind port number")]
    pub listen_port: Option<u16>,
    #[clap(long, short = 'u', help = "Use unix socket for communication layer")]
    pub unix_socket: Option<String>,
    #[clap(
        long,
        short = 'e',
        default_value = "libx264",
        help = "Encoder name. Ex: libx264, h264_qsv, hevc_nvenc"
    )]
    pub encoder: String,
    #[clap(
        long,
        short = 'a',
        default_value_t = false,
        help = "Allow audio forwarding from server to client"
    )]
    pub audio: bool,
    #[clap(
        long,
        short = 'k',
        help = "Use video source from file instead of video server api"
    )]
    pub extern_img_source: Option<String>,
    #[clap(long, short = 'y', help = "Video source is xwd formated")]
    pub source_is_xwd: bool,
    #[clap(
        long,
        short = 'd',
        default_value_t = false,
        help = "Loop if client disconnect instead of quitting"
    )]
    #[clap(
        long,
        default_value_t = false,
        help = "Disable client to server clipboard"
    )]
    pub disable_client_clipboard: bool,
    #[clap(
        long,
        default_value_t = false,
        help = "Disable server to client clipboard"
    )]
    pub disable_server_clipboard: bool,
    pub keep_listening: bool,
    #[clap(long, help = "Displays protocol version")]
    pub proto: bool,
    #[clap(short='v', long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

const MAX_HEADER_SIZE: u32 = 0x100;
const MAX_NCOLORS: u32 = 0x100;
pub const MAX_WINDOW_WIDTH: u32 = 8129;
pub const MAX_WINDOW_HEIGHT: u32 = 8129;
pub const MAX_BYTES_PER_LINE: u32 = MAX_WINDOW_WIDTH * 4;

pub const MAX_CURSOR_WIDTH: u32 = 1024;
pub const MAX_CURSOR_HEIGHT: u32 = 1024;

const OFFSET_NCOLORS: usize = 19;
const OFFSET_BYTES_PER_LINE: usize = 12;
const OFFSET_WINDOW_WIDTH: usize = 20;
const OFFSET_WINDOW_HEIGHT: usize = 21;

/// Get offset of pixels data from an Xwd image
pub fn get_xwd_data(data: &[u8]) -> Result<(&[u8], u32, u32, u32)> {
    let header = data.get(0..0x100).expect("Cannot get source header");
    let header_size = BigEndian::read_u32(&header[0..4]);
    if header_size > MAX_HEADER_SIZE {
        return Err(anyhow!(format!("Strange header size {header_size:x}")));
    }
    let ncolors = BigEndian::read_u32(&header[OFFSET_NCOLORS * 4..(OFFSET_NCOLORS + 1) * 4]);
    if ncolors > MAX_NCOLORS {
        return Err(anyhow!(format!("Strange ncolors {header_size:x}")));
    }
    let bytes_per_line =
        BigEndian::read_u32(&header[OFFSET_BYTES_PER_LINE * 4..(OFFSET_BYTES_PER_LINE + 1) * 4]);

    if bytes_per_line > MAX_BYTES_PER_LINE {
        return Err(anyhow!(format!(
            "Strange bytes_per_line {bytes_per_line:x}",
        )));
    }
    let window_x =
        BigEndian::read_u32(&header[OFFSET_WINDOW_WIDTH * 4..(OFFSET_WINDOW_WIDTH + 1) * 4]);

    if window_x > MAX_WINDOW_WIDTH {
        return Err(anyhow!(format!("Strange window_x {window_x:x}")));
    }
    let window_y =
        BigEndian::read_u32(&header[OFFSET_WINDOW_HEIGHT * 4..(OFFSET_WINDOW_HEIGHT + 1) * 4]);

    if window_y > MAX_WINDOW_HEIGHT {
        return Err(anyhow!(format!("Strange window_y {window_y:x}")));
    }

    let offset = header_size + ncolors * 0xc;
    let size = bytes_per_line * window_y;
    trace!("Grab from extern source {:?}", size);
    Ok((
        data.get(offset as usize..(offset + size) as usize)
            .expect("Cannot get source"),
        window_x,
        window_y,
        bytes_per_line,
    ))
}

/// Logger uses env var to set default log level
/// Change log level according to verbose option
pub fn init_logger(level: u8) {
    let mut log_builder = env_logger::Builder::from_default_env();
    log_builder.format_timestamp_nanos();
    match level {
        0 => {}
        1 => {
            log_builder.filter_level(log::LevelFilter::Warn);
        }
        2 => {
            log_builder.filter_level(log::LevelFilter::Info);
        }
        3 => {
            log_builder.filter_level(log::LevelFilter::Debug);
        }
        _ => {
            log_builder.filter_level(log::LevelFilter::Trace);
        }
    }

    log_builder.init();
}

#[derive(Parser, Debug)]
pub struct ProtoArgs {
    #[clap(long, default_value_t = false, help = "Display proto version")]
    pub proto: bool,
}

/// Test if the only argument is --proto
/// TODO: what we want here is an argument which can override mandatory positional arguments
/// The goal is to allow for example "sanzu_client --proto" even if sanzu_client
/// needs 2 positional arguments. To do this, we build a dummy clap parser with
/// only one argument.
/// Don't forget to add proto in the real command line.
pub fn is_proto_arg() -> bool {
    if let Ok(matches) = ProtoArgs::command().try_get_matches() {
        if *matches.get_one::<bool>("proto").unwrap() {
            return true;
        }
    }
    false
}
