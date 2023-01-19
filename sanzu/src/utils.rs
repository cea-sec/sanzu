use anyhow::Result;
use byteorder::{BigEndian, ByteOrder};
use std::net::IpAddr;

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

pub struct ArgumentsSrv<'a> {
    pub vsock: bool,
    pub stdio: bool,
    pub unixsock: bool,
    pub connect_unixsock: bool,
    pub address: &'a str,
    pub port: &'a str,
    pub encoder_name: String,
    pub seamless: bool,
    pub keep_server_resolution: bool,
    pub audio: bool,
    pub raw_sound: bool,
    pub export_video_pci: bool,
    pub restrict_clipboard: bool,
    pub extern_img_source: Option<String>,
    pub avoid_img_extraction: bool,
    pub rdonly: bool,
    pub endless_loop: bool,
}

pub struct ArgumentsClient<'a> {
    pub address: &'a str,
    pub port: u16,
    pub audio: bool,
    pub audio_sample_rate: Option<u32>,
    pub audio_buffer_ms: u32,
    pub server_cname: Option<String>,
    pub tls_ca: Option<String>,
    pub tls_server_name: Option<String>,
    pub client_cert: Option<String>,
    pub client_key: Option<String>,
    pub login: bool,
    pub clipboard_config: ClipboardConfig,
    pub window_mode: bool,
    pub decoder_name: Option<String>,
    pub printdir: Option<String>,
    pub proxycommand: Option<String>,
    pub sync_key_locks: bool,
    pub video_shared_mem: Option<String>,
    pub shm_is_xwd: bool,
}

pub struct ArgumentsProxy<'a> {
    pub vsock: bool,
    pub server_addr: &'a str,
    pub server_port: &'a str,
    pub listen_address: IpAddr,
    pub listen_port: Option<u16>,
    pub unix_socket: Option<String>,
    pub encoder_name: String,
    pub audio: bool,
    pub video_shared_mem: Option<String>,
    pub shm_is_xwd: bool,
    pub endless_loop: bool,
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
        return Err(anyhow!(format!("Strange header size {:x}", header_size)));
    }
    let ncolors = BigEndian::read_u32(&header[OFFSET_NCOLORS * 4..(OFFSET_NCOLORS + 1) * 4]);
    if ncolors > MAX_NCOLORS {
        return Err(anyhow!(format!("Strange ncolors {:x}", header_size)));
    }
    let bytes_per_line =
        BigEndian::read_u32(&header[OFFSET_BYTES_PER_LINE * 4..(OFFSET_BYTES_PER_LINE + 1) * 4]);

    if bytes_per_line > MAX_BYTES_PER_LINE {
        return Err(anyhow!(format!(
            "Strange bytes_per_line {:x}",
            bytes_per_line
        )));
    }
    let window_x =
        BigEndian::read_u32(&header[OFFSET_WINDOW_WIDTH * 4..(OFFSET_WINDOW_WIDTH + 1) * 4]);

    if window_x > MAX_WINDOW_WIDTH {
        return Err(anyhow!(format!("Strange window_x {:x}", window_x)));
    }
    let window_y =
        BigEndian::read_u32(&header[OFFSET_WINDOW_HEIGHT * 4..(OFFSET_WINDOW_HEIGHT + 1) * 4]);

    if window_y > MAX_WINDOW_HEIGHT {
        return Err(anyhow!(format!("Strange window_y {:x}", window_y)));
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
