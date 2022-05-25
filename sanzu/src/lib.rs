#[macro_use]
extern crate log;

#[cfg(windows)]
#[macro_use]
extern crate lazy_static;

#[macro_use]
extern crate anyhow;

#[macro_use]
extern crate sanzu_common;

#[cfg(feature = "kerberos")]
pub mod auth_kerberos;
pub mod client;
pub mod client_utils;
#[cfg(windows)]
pub mod client_wind3d;
#[cfg(unix)]
pub mod client_x11;
pub mod ffmpeg_helper;
pub mod server_utils;
#[cfg(unix)]
pub use x11_clipboard;
pub mod utils;
#[cfg(windows)]
pub mod utils_win;
#[cfg(unix)]
pub mod utils_x11;
#[cfg(windows)]
pub use client_wind3d as client_graphics;
#[cfg(unix)]
pub use client_x11 as client_graphics;
pub mod config;
//pub mod proto;
#[cfg(unix)]
pub mod proxy;
#[cfg(windows)]
pub mod proxy_windows;
pub mod server;
#[cfg(windows)]
pub mod server_windows;
#[cfg(unix)]
pub mod server_x11;
#[cfg(windows)]
pub use proxy_windows as proxy;
pub mod osd;
pub mod sound;
#[cfg(windows)]
pub mod sspi;
pub mod video_decoder;
pub mod video_encoder;
pub mod yuv_rgb_rs;
