use anyhow::{Context, Result};

#[cfg(all(unix, feature = "kerberos"))]
use sanzu_common::auth_kerberos::do_kerberos_client_auth;
#[cfg(target_family = "unix")]
use sanzu_common::auth_pam::do_pam_auth;
#[cfg(target_family = "unix")]
use sanzu_common::Stdio;
use sanzu_common::{
    proto::{recv_client_msg_or_error, send_server_err_event, VERSION},
    tls_helper::{get_subj_alt_names, make_server_config, tls_do_handshake},
    tunnel,
    utils::get_username_from_principal,
    ReadWrite, Tunnel,
};

use serde::{Deserialize, Serialize};

use spin_sleep::LoopHelper;
use std::{
    net::{self, IpAddr, TcpListener},
    sync::Mutex,
    time::Instant,
};

#[cfg(unix)]
use std::io::{self, BufRead, Write};

#[cfg(unix)]
use std::{
    sync::mpsc::{channel, Sender},
    thread::{self},
};

#[cfg(unix)]
use vsock;

#[cfg(target_family = "unix")]
use crate::config::AuthType;
use crate::{
    config::{ConfigServer, ConfigTls},
    sound::SoundEncoder,
    utils::{ArgumentsSrv, ServerEvent},
    video_encoder::{get_encoder_category, init_video_encoder, Encoder},
};
use rustls::ServerConnection;

use x509_parser::prelude::*;

#[cfg(unix)]
use crate::server_x11::init_x11rb;

#[cfg(windows)]
use crate::server_windows::init_win;

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
struct ServerStats {
    fps: u64,
    frame_time: u64,
    grab: u64,
    enc: u64,
    send: u64,
    recv: u64,
    size: (u16, u16),
}

lazy_static! {
    static ref SERVER_STATS: Mutex<ServerStats> = Mutex::new(ServerStats::default());
}

/// Tls auth / Kerberos Auth
fn auth_client(
    config_tls: &ConfigTls,
    socket: &mut Box<dyn ReadWrite>,
) -> Result<(ServerConnection, Option<String>)> {
    let tls_config = make_server_config(
        &config_tls.ca_file,
        &config_tls.auth_cert,
        &config_tls.auth_key,
        config_tls.allowed_client_domains.is_some(),
    )
    .context("Cannot make tls config")?;
    debug!("Using tls");

    let mut tls_conn =
        ServerConnection::new(tls_config).context("Error in new ServerConnection")?;

    let username = if let Some(ref allowed_client_domains) = config_tls.allowed_client_domains {
        if allowed_client_domains.is_empty() {
            warn!("TLS allowed domains list is empty");
        }

        tls_do_handshake(&mut tls_conn, socket).context("Error in tls_do_handshake")?;
        let certs = tls_conn.peer_certificates();
        let certs = certs
            .map(Ok)
            .unwrap_or_else(|| Err(anyhow!("No cert from user")))?;
        let cert = certs
            .last()
            .map(Ok)
            .unwrap_or_else(|| Err(anyhow!("No cert from user")))?;
        let (_data, cert) =
            X509Certificate::from_der(&cert.0).context("Error in X509Certificate from der")?;

        let subj_alt_name =
            get_subj_alt_names(&cert).context("Error in get subject alternative name")?;
        debug!("Alt name: {:?}", subj_alt_name);

        let username = get_username_from_principal(&subj_alt_name, allowed_client_domains)
            .context("Principal doesnt match realm pattern")?;

        Some(username)
    } else {
        None
    };

    info!("Authenticated user: {:?}", username);
    Ok((tls_conn, username))
}

#[cfg(unix)]
/// Handle control api
/// restart => restart encoder
/// stats => send encoding stats
fn control_api(control_path: &str, control_sender: Sender<()>) {
    let pid = std::process::id();
    let control_path = control_path.replace("%PID%", &format!("{pid}"));
    // Try to remove path first
    let _ = std::fs::remove_file(&control_path);
    let listener = std::os::unix::net::UnixListener::bind(&control_path)
        .unwrap_or_else(|_| panic!("Cannot bind {:?}", control_path));
    loop {
        let (mut client, addr) = listener.accept().expect("Error in UnixListener accept");
        info!("Client {:?}", addr);
        let control_sender_cp = control_sender.clone();
        thread::spawn(move || {
            let mut command = String::new();
            if let Ok(length) = io::BufReader::new(&mut client).read_line(&mut command) {
                info!("Command: {:?} {}", command, length);
                match command.trim_end() {
                    "restart" => {
                        info!("Restart encoder requested");
                        control_sender_cp.send(()).expect("Cannot send control");
                        if client.write_all("Ok".as_bytes()).is_err() {
                            warn!("Cannot send ok");
                        }
                    }
                    "stats" => {
                        info!("Stats requested");
                        let stats = SERVER_STATS.lock().unwrap().clone();

                        if let Ok(stats_str) = serde_json::to_string(&stats) {
                            if client.write_all(stats_str.as_bytes()).is_err() {
                                warn!("Cannot send stat");
                            }
                        } else {
                            warn!("Cannot generate stats");
                        }
                    }
                    _ => {
                        error!("Unknown command");
                        if client.write_all("Unknown command".as_bytes()).is_err() {
                            warn!("Cannot send stat");
                        }
                    }
                }
            }
        });
    }
}

/// Exec main loop
///
pub fn run(config: &ConfigServer, arguments: &ArgumentsSrv) -> Result<()> {
    if arguments.endless_loop {
        loop {
            if let Err(err) = run_server(config, arguments) {
                error!("Server error");
                err.chain().for_each(|cause| error!(" - due to {}", cause));
            }
        }
    } else {
        run_server(config, arguments)
    }
}

/// Server main loop
///
/// The loop is composed of the following actions:
/// - retrieve server sound
/// - grab server frame
/// - poll graphic server events
/// - encode image
/// - serialize / send events to client
/// - receive / handle client events
pub fn run_server(config: &ConfigServer, arguments: &ArgumentsSrv) -> Result<()> {
    info!("Start server");

    let mut sock: Box<dyn ReadWrite> = match (arguments.vsock, arguments.stdio, arguments.unixsock)
    {
        (true, false, false) => {
            #[cfg(unix)]
            {
                let port = arguments
                    .port
                    .parse::<u32>()
                    .context(format!("Error in vsock port parsing {}", arguments.port))?;
                let address = arguments.address.parse::<u32>().context(format!(
                    "Error in vsock address parsing {}",
                    arguments.address
                ))?;
                let listener = vsock::VsockListener::bind(&vsock::VsockAddr::new(address, port))
                    .context(format!("Error in VsockListener {address} {port}"))?;
                let (socket, addr) = listener.accept().context("failed to accept connection")?;
                info!("Client {:?}", addr);
                Box::new(socket)
            }
            #[cfg(windows)]
            {
                return Err(anyhow!("Vsock not supported on windows"));
            }
        }
        (false, false, true) => {
            #[cfg(unix)]
            {
                let socket = if arguments.connect_unixsock {
                    std::os::unix::net::UnixStream::connect(arguments.address)?
                } else {
                    let listener = std::os::unix::net::UnixListener::bind(arguments.address)?;
                    let (socket, addr) =
                        listener.accept().context("Error in UnixListener accept")?;
                    info!("Client {:?}", addr);
                    socket
                };
                Box::new(socket)
            }
            #[cfg(windows)]
            {
                return Err(anyhow!("Unix sockets are not supported on windows"));
            }
        }
        (false, true, false) => {
            #[cfg(unix)]
            {
                Box::new(Stdio {})
            }
            #[cfg(windows)]
            {
                return Err(anyhow!("STDIO is not supported on windows"));
            }
        }
        (false, false, false) => {
            let port = arguments
                .port
                .parse::<u16>()
                .context(format!("Cannot parse port {:?}", arguments.port))?;
            let address = arguments
                .address
                .parse::<IpAddr>()
                .context(format!("Error ip in parsing {:?}", arguments.address))?;
            let listener = TcpListener::bind(net::SocketAddr::new(address, port))?;
            let (socket, addr) = listener
                .accept()
                .context(format!("Error in TcpListener {address} {port}"))?;
            socket.set_nodelay(true)?;
            info!("Client {:?}", addr);
            Box::new(socket)
        }
        _ => {
            return Err(anyhow!("vsock / stdio / unixsock arguments error"));
        }
    };

    let (mut tls_conn, _tls_username) = match &config.tls {
        Some(config_tls) => {
            let (tls_conn, username) =
                auth_client(config_tls, &mut sock).context("Error in auth client")?;
            (Some(tls_conn), username)
        }
        None => (None, None),
    };

    let (mut sock, has_tls): (Box<dyn ReadWrite>, bool) = match tls_conn.as_mut() {
        Some(tls_conn) => {
            let conn = rustls::Stream::new(tls_conn, &mut sock);
            (Box::new(conn), true)
        }
        None => (Box::new(sock), false),
    };

    #[cfg(windows)]
    info!("Tls state: {}", has_tls);

    // Send client version
    let server_version = tunnel::Version {
        version: VERSION.to_owned(),
    };
    send_server_msg_type!(&mut sock, server_version, Version).context("Error in send Version")?;

    /* Recv client version */
    let client_version: tunnel::Version =
        recv_client_msg_type!(&mut sock, Version).context("Error in send client version")?;

    info!("Client version {:?}", client_version);
    if client_version.version != VERSION {
        return Err(anyhow!(
            "Version mismatch server: {:?} client: {:?}",
            VERSION,
            client_version.version
        ));
    }

    #[cfg(target_family = "unix")]
    if let Some(auth_type) = &config.auth_type {
        match auth_type {
            #[cfg(all(unix, feature = "kerberos"))]
            AuthType::Kerberos(realms) => {
                if realms.is_empty() {
                    warn!("Kerberos allowed realms list is empty");
                }

                let username = do_kerberos_client_auth(realms, &mut sock)?;
                info!("Kerberos authentication ok for user: {}", username);
            }
            #[cfg(target_family = "unix")]
            AuthType::Pam(pam_name) => {
                if !has_tls {
                    warn!("Use of pam without Tls detected!");
                }
                let username =
                    do_pam_auth(&mut sock, pam_name).context("Error in pam authentication")?;
                info!("Pam authentication ok for user: {}", username);
            }
        }
    }
    let codec_name = get_encoder_category(&arguments.encoder_name)?;

    /* Send server hello with image info & codec name */
    let (mut server_info, audio_sample_rate) =
        if arguments.keep_server_resolution || arguments.rdonly {
            #[cfg(unix)]
            let server_info = init_x11rb(arguments, config, None).context("Cannot init_x11rb")?;
            #[cfg(windows)]
            let server_info = init_win(arguments, config, None)?;

            let (screen_width, screen_height) = server_info.size();
            let server_mode = tunnel::server_hello::Msg::Fullscreen(tunnel::ServerFullScreen {
                width: screen_width as u32,
                height: screen_height as u32,
            });

            let server_hello = tunnel::ServerHello {
                codec_name,
                audio: arguments.audio,
                msg: Some(server_mode),
            };

            send_server_msg_type!(&mut sock, server_hello, Hello).context("Cannot send hello")?;

            /* recv client hello with audio bool */
            let msg: tunnel::ClientHelloFullscreen =
                recv_client_msg_type!(&mut sock, Clienthellofullscreen)
                    .context("Error in send client hello full screen")?;

            let audio_sample_rate = match msg.audio {
                true => Some(msg.audio_sample_rate),
                false => None,
            };
            (server_info, audio_sample_rate)
        } else {
            let server_mode = tunnel::server_hello::Msg::AdaptScreen(tunnel::ServerAdaptScreen {
                seamless: arguments.seamless,
            });

            let server_hello = tunnel::ServerHello {
                codec_name,
                audio: arguments.audio,
                msg: Some(server_mode),
            };

            send_server_msg_type!(&mut sock, server_hello, Hello).context("Cannot send hello")?;

            /* recv client hello with audio bool */
            let msg: tunnel::ClientHelloResolution =
                recv_client_msg_type!(&mut sock, Clienthelloresolution)
                    .context("Error in recv client hello resolution")?;

            info!("Client screen size {:?}x{:?}", msg.width, msg.height);
            let client_screen_size = Some((msg.width as u16, msg.height as u16));
            #[cfg(unix)]
            let mut server_info =
                init_x11rb(arguments, config, client_screen_size).context("Cannot init_x11rb")?;
            #[cfg(windows)]
            let mut server_info = init_win(arguments, config, client_screen_size)?;

            // Force server resolution
            let (width, height) = server_info.size();
            let (width, height) = (width & !1, height & !1);
            if server_info
                .change_resolution(config, width as u32, height as u32)
                .is_err()
            {
                warn!("Cannot change server resolution");
            }

            let audio_sample_rate = match msg.audio {
                true => Some(msg.audio_sample_rate),
                false => None,
            };
            (server_info, audio_sample_rate)
        };

    let mut video_encoder: Box<dyn Encoder> = init_video_encoder(
        arguments.encoder_name.as_str(),
        config.ffmpeg_options(None),
        config.ffmpeg_options(Some(arguments.encoder_name.as_str())),
        &config.video.ffmpeg_options_cmd,
        server_info.size(),
    )
    .context("Error in init video encoder")
    .map_err(|err| send_server_err_event(&mut sock, err))?;

    let mut sound_obj = match (audio_sample_rate, arguments.audio) {
        (Some(audio_sample_rate), true) => {
            match SoundEncoder::new(
                "default",
                arguments.raw_sound,
                audio_sample_rate,
                config.audio.max_buffer_ms,
            ) {
                Ok(mut sound_obj) => {
                    sound_obj.start()?;
                    Some(sound_obj)
                }
                Err(err) => {
                    error!("Error in sound encoder init: {:?}", err);
                    None
                }
            }
        }
        _ => None,
    };

    let mut prev_time_start = Instant::now();

    let mut loop_helper = LoopHelper::builder().build_with_target_rate(config.video.max_fps as f64); // limit FPS if possible

    let mut new_size = None;
    let (width, height) = server_info.size();
    let mut cur_size = Some((width as u32, height as u32));

    // Do socket control
    #[cfg(unix)]
    let (control_sender, control_receiver) = channel();
    #[cfg(unix)]
    {
        if let Some(control_path) = config.video.control_path.as_ref().cloned() {
            info!("Listening on control path {:?}", control_path);
            thread::spawn(move || {
                control_api(&control_path.clone(), control_sender);
            });
        }
    }

    let mut msg_stats = "".to_owned();
    let err = loop {
        let time_start = Instant::now();
        loop_helper.loop_start();

        let mut events = vec![];
        if let Some((width, height)) = new_size.take() {
            // Change resolution if:
            // - requested resolution has really changed
            // - width or height is not null
            // - we are allowed to change resolution
            if Some((width, height)) != cur_size
                && width != 0
                && height != 0
                && !arguments.keep_server_resolution
            {
                match server_info.change_resolution(config, width, height) {
                    Ok(_) => {
                        cur_size = Some((width, height));
                        // Create new encoder only if we change resolution
                        debug!("New codec {}x{}", width, height);
                        video_encoder = video_encoder
                            .change_resolution(width, height)
                            .context("Cannot change codec resolution")?;
                        let msg = tunnel::EventDisplay { width, height };
                        let msg = tunnel::MessageSrv {
                            msg: Some(tunnel::message_srv::Msg::Display(msg)),
                        };
                        events.push(msg);
                    }
                    Err(err) => {
                        warn!("Error in change_resolution");
                        err.chain().for_each(|cause| error!(" - due to {}", cause));
                    }
                }
            }
        }

        // Test is we receiver control message
        #[cfg(unix)]
        {
            let mut control_msg = false;
            while control_receiver.try_recv().is_ok() {
                control_msg = true;
            }
            if control_msg {
                info!("Received control msg");
                video_encoder = video_encoder
                    .reload()
                    .context("Cannot reload encoder in control management")?;
            }
        }

        // Pump sound to encoder
        if let Some(ref mut sound_obj) = sound_obj {
            sound_obj.read_sound();
        }

        /* Grab frame */
        if let Err(err) = server_info.grab_frame() {
            error!("grab fail {:?}", err);
            break anyhow!("Grab fail: {}", err);
        }

        let time_grab = Instant::now();

        /* Manage clipboard events */
        match server_info.poll_events() {
            Ok(mut new_events) => events.append(&mut new_events),
            Err(err) => {
                break anyhow!("Poll error: {}", err);
            }
        };

        let time_event = Instant::now();

        let (mut img_events, timings) = server_info
            .generate_encoded_img(&mut video_encoder)
            .context("Error in generate_encoded_img")?;
        let time_encode = Instant::now();

        let mut sound_events = if let Some(ref mut sound_obj) = sound_obj {
            sound_obj.recv_events()
        } else {
            vec![]
        };
        events.append(&mut sound_events);

        let time_sound = Instant::now();

        events.append(&mut img_events);

        /* Send stats */
        let msg = tunnel::message_srv::Msg::Stats(tunnel::EventStats { stats: msg_stats });
        let msg = tunnel::MessageSrv { msg: Some(msg) };
        events.push(msg);

        /* Send events */
        send_server_msg_type!(&mut sock, tunnel::MessagesSrv { msgs: events }, Msgssrv)
            .context("Cannot send events")?;

        let time_send = Instant::now();

        let msgs =
            recv_client_msg_type!(&mut sock, Msgsclient).context("Cannot recv client msgs")?;

        if !arguments.rdonly {
            let server_events = server_info
                .handle_client_event(msgs)
                .context("Error in client handle events")?;

            for server_event in server_events {
                match server_event {
                    ServerEvent::ResolutionChange(width, height) => {
                        let width = width & !1;
                        let height = height & !1;
                        // Keep change resolution event for next cycle
                        new_size = Some((width, height));
                    }
                }
            }
        }

        let mut timings_str = String::new();
        if let Some(timings) = timings {
            timings_str += &timings
                .times
                .iter()
                .map(|(name, value)| format!("{name} {value:>7.1?}"))
                .collect::<Vec<String>>()
                .join(" ");
        }

        let time_stop = Instant::now();
        let frame_time = time_start - prev_time_start;
        let frame_time_micro = frame_time.as_micros();
        let fps = if frame_time_micro == 0 {
            "-".to_owned()
        } else {
            format!("{:3}", 1_000_000 / frame_time_micro)
        };

        let msg = format!(
                "Fps:{} Frame time: {:>7} Total: {:>7} grab: {:>7} event: {:>7} encode: {:>7} ({}) sound: {:>7} send: {:>7} recv: {:>7}",
                fps,
                &format!("{:.1?}", (time_start - prev_time_start)),
                &format!("{:.1?}", time_stop - time_start),
                &format!("{:.1?}", time_grab - time_start),
                &format!("{:.1?}", time_event - time_grab),
                &format!("{:.1?}", time_encode - time_event),
                &timings_str,
                &format!("{:.1?}", time_sound - time_encode),
                &format!("{:.1?}", time_send - time_sound),
                &format!("{:.1?}", time_stop - time_send),
            );
        debug!("{}", msg);
        msg_stats = msg;
        let fps = match frame_time_micro as u64 {
            0 => 0,
            micros => 1_000_000 / micros,
        };
        let size = match cur_size.as_ref() {
            Some((width, height)) => (*width as u16, *height as u16),
            None => (0, 0),
        };
        *SERVER_STATS.lock().unwrap() = ServerStats {
            fps,
            frame_time: (time_start - prev_time_start).as_millis() as u64,
            grab: (time_grab - time_start).as_millis() as u64,
            enc: (time_encode - time_event).as_millis() as u64,
            send: (time_send - time_sound).as_millis() as u64,
            recv: (time_stop - time_send).as_millis() as u64,
            size,
        };

        prev_time_start = time_start;
        loop_helper.loop_sleep(); // sleeps to acheive target FPS rate
    };

    Err(err)
}
