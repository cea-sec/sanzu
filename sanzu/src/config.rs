use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs::File, io, io::Read, path::Path};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConfigTls {
    pub server_name: String,
    pub ca_file: String,
    pub crl_file: Option<String>,
    pub ocsp_file: Option<String>,
    pub auth_cert: String,
    pub auth_key: String,
    /// List of domains to authenticate clients
    pub allowed_client_domains: Option<Vec<String>>,
}

/// Holds configuration for the video frame behavior
#[derive(Debug, Serialize, Deserialize)]
pub struct Video {
    /// Max frame rate
    pub max_fps: u64,
    /// Max identical frames to wait before stopping sending frame to the encoder / client.
    ///
    /// If the server see that the graphic has not changed until, for example 10
    /// frames, the server will stop sending frames to the encoder (and thus
    /// send empty frame to the client).
    ///
    /// This has two advantages:
    /// - saves bandwidth
    /// - saves encoder cpu/gpu time
    ///
    /// Drawback: Some encoders (h264, ...) don't send the full resolution on
    /// the first frame but tend to send more and more details over time. So a
    /// too little number of frames will result in a client graphic window with
    /// very little details, and stall like this until the graphic change.
    pub max_stall_img: u32,
    /// Holds the command line to execute to retreive special ffmpeg options to
    /// apply to the codec (which can be generated dynamically
    pub ffmpeg_options_cmd: Option<String>,
    /// Socket control path
    pub control_path: Option<String>,
}

/// Holds configuration for the audio timings
#[derive(Debug, Serialize, Deserialize)]
pub struct Audio {
    /// Server side buffer size
    pub max_buffer_ms: u64,
}

/// Holds configuration for the shm video export
#[derive(Debug, Serialize, Deserialize)]
pub struct ExportVideoPci {
    /// PCI subsystem vendor
    /// Ex: for virtio, 0x1af4
    pub vendor: String,
    /// PCI device
    /// Ex: for share ram, 0x1110
    pub device: String,
}

/// Support authentication mecanism
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", content = "args")]
pub enum AuthType {
    #[cfg(all(unix, feature = "kerberos"))]
    // List of allowed realms
    Kerberos(Vec<String>),
    #[cfg(target_family = "unix")]
    // Pam name
    Pam(String),
}

/// Server configuration
#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigServer {
    pub video: Video,
    pub audio: Audio,
    pub export_video_pci: Option<ExportVideoPci>,
    pub tls: Option<ConfigTls>,
    pub auth_type: Option<AuthType>,
    /// Holds codecs configuration.
    ///
    /// For each codec name, stores the HashMap which links codec property to
    /// its value
    ffmpeg: HashMap<String, HashMap<String, String>>,
}

impl ConfigServer {
    pub fn ffmpeg_options(
        &self,
        codec: Option<&str>,
    ) -> Option<impl Iterator<Item = (&String, &String)> + '_> {
        self.ffmpeg
            .get(codec.unwrap_or("global"))
            .map(|opts| opts.iter())
    }
}

/// Read configuration from a file
pub fn read_server_config<P: AsRef<Path>>(path: P) -> io::Result<ConfigServer> {
    let mut content = String::new();
    File::open(path)?.read_to_string(&mut content)?;
    toml::from_str(&content).map_err(|err| io::Error::new(io::ErrorKind::Other, err))
}

/// Server configuration
#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigClient {
    /// Holds codecs configuration.
    ///
    /// For each codec name, stores the HashMap which links codec property to
    /// its value
    pub ffmpeg: HashMap<String, HashMap<String, String>>,
}

impl ConfigClient {
    pub fn ffmpeg_options(
        &self,
        codec: Option<&str>,
    ) -> Option<impl Iterator<Item = (&String, &String)> + '_> {
        self.ffmpeg
            .get(codec.unwrap_or("global"))
            .map(|opts| opts.iter())
    }
}

/// Read configuration from a file
pub fn read_client_config<P: AsRef<Path>>(path: P) -> io::Result<ConfigClient> {
    let mut content = String::new();
    File::open(path)?.read_to_string(&mut content)?;
    toml::from_str(&content).map_err(|err| io::Error::new(io::ErrorKind::Other, err))
}

#[cfg(test)]
mod tests {
    use super::*;
    use toml;

    const CONF: &'static str = r#"
[video]
max_fps = 60
max_stall_img = 30

[audio]
sample_rate = 44100
max_buffer_ms = 200

[ffmpeg.global]
b = "2000000"

[ffmpeg.libx264]
pixel_format = "yuv444p"
preset = "fast"
tune = "zerolatency"
"#;

    #[test]
    fn test_conf() {
        let config: ConfigServer = toml::from_str(&CONF).unwrap();
        dbg!(&config);
    }
}
