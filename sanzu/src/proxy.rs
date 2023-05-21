use anyhow::{Context, Result};

use crate::{
    config::ConfigServer,
    sound::{encode_sound, SOUND_FREQ},
    utils::{
        get_xwd_data, ProxyArgsConfig, MAX_BYTES_PER_LINE, MAX_WINDOW_HEIGHT, MAX_WINDOW_WIDTH,
    },
    video_encoder::{get_encoder_category, init_video_encoder},
};
use byteorder::{LittleEndian, ReadBytesExt};
use memmap2::MmapOptions;
use sanzu_common::{
    proto::{recv_client_msg_or_error, recv_server_msg_or_error, VERSION},
    tunnel, ReadWrite, Tunnel,
};

use std::{
    fmt::Write as _,
    fs,
    io::Cursor,
    net::{SocketAddr, TcpListener, TcpStream},
    os::unix::net::UnixStream,
    sync::mpsc::channel,
    thread::{self},
    time::Instant,
};

/// Send whole srv chain error
fn send_srv_err_event(sock: &mut Box<dyn ReadWrite>, err: anyhow::Error) -> anyhow::Error {
    let mut errors = vec!["Errors from proxy's server:".to_string()];
    for err in err.chain() {
        errors.push(format!("    {err}"));
    }

    let err_msg = tunnel::EventError { errors };
    let srv_err_msg = tunnel::ServerMsgOrErr {
        msg: Some(tunnel::server_msg_or_err::Msg::Err(err_msg)),
    };

    if let Err(err) = Tunnel::send(sock, srv_err_msg) {
        anyhow!("Error in send: Peer has closed connection? ({:?})", err)
    } else {
        err
    }
}

/// Send whole client chain error
fn send_client_err_event(sock: &mut Box<dyn ReadWrite>, err: anyhow::Error) -> anyhow::Error {
    let mut errors = vec!["Errors from proxy's client:".to_string()];
    for err in err.chain() {
        errors.push(format!("    {err}"));
    }

    let err_msg = tunnel::EventError { errors };
    let srv_err_msg = tunnel::ClientMsgOrErr {
        msg: Some(tunnel::client_msg_or_err::Msg::Err(err_msg)),
    };

    if let Err(err) = Tunnel::send(sock, srv_err_msg) {
        anyhow!("Error in send: Peer has closed connection? ({:?})", err)
    } else {
        err
    }
}

fn recv_srv_msg_or_error(stream: &mut dyn ReadWrite) -> Result<tunnel::message_server_ok::Msg> {
    let msg: tunnel::ServerMsgOrErr = Tunnel::recv(stream).context("Error in recv pkt")?;
    match msg.msg {
        Some(tunnel::server_msg_or_err::Msg::Ok(msg_ok)) => {
            // Message is ok
            if let Some(msg) = msg_ok.msg {
                Ok(msg)
            } else {
                Err(anyhow!("Empty pkt from server"))
            }
        }
        Some(tunnel::server_msg_or_err::Msg::Err(msg_err)) => {
            // Message is err
            let mut error = Err(anyhow!("[end err]"));
            for err in msg_err.errors.iter().rev() {
                error = error.context(err.to_string());
            }
            error = error.context("Error from proxy's server");
            error
        }
        _ => Err(anyhow!("Bad pkt from server")),
    }
}

macro_rules! recv_srv_msg_type {
    (
        $sock: expr, $name: ident
    ) => {{
        match recv_srv_msg_or_error($sock) {
            Err(err) => Err(err.context(anyhow!("Received error msg"))),
            Ok(msg) => {
                if let tunnel::message_server_ok::Msg::$name(msg) = msg {
                    Ok(msg)
                } else {
                    Err(anyhow!("Bad packet type"))
                }
            }
        }
    }};
}

macro_rules! recv_client_msg_type {
    (
        $sock: expr, $name: ident
    ) => {{
        match recv_client_msg_or_error($sock) {
            Err(err) => Err(err.context(anyhow!("Received error msg"))),
            Ok(msg) => {
                if let tunnel::message_client_ok::Msg::$name(msg) = msg {
                    Ok(msg)
                } else {
                    Err(anyhow!("Bad packet type"))
                }
            }
        }
    }};
}

macro_rules! send_srv_msg_type {
    (
        $sock: expr, $msg: expr, $name: ident
    ) => {{
        let msg_ok = tunnel::MessageServerOk {
            msg: Some(tunnel::message_server_ok::Msg::$name($msg)),
        };
        let msgsrv_ok = tunnel::ServerMsgOrErr {
            msg: Some(tunnel::server_msg_or_err::Msg::Ok(msg_ok)),
        };
        Tunnel::send($sock, msgsrv_ok).context("Error in send: Peer has closed connection?")
    }};
}

/// Exec main loop
///
pub fn run(config: &ConfigServer, arguments: &ProxyArgsConfig) -> Result<()> {
    if arguments.keep_listening {
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

pub fn run_server(config: &ConfigServer, arguments: &ProxyArgsConfig) -> Result<()> {
    let codec_name = get_encoder_category(&arguments.encoder)?;

    let mut sound_encoder = opus::Encoder::new(
        SOUND_FREQ,
        opus::Channels::Mono,
        opus::Application::LowDelay,
    )
    .expect("Cannot create sound encoder");

    let video_shared_mem = match arguments.extern_img_source.as_deref() {
        Some(extern_img_source) => {
            let file = fs::File::open(extern_img_source)
                .context(format!("Error in open shared mem {extern_img_source:?}"))?;
            unsafe {
                Some(
                    MmapOptions::new()
                        .map(&file)
                        .context("Error in map shared mem")?,
                )
            }
        }
        None => None,
    };

    /* wait for client */
    let mut client: Box<dyn ReadWrite> =
        match (arguments.listen_port, arguments.unix_socket.as_deref()) {
            (listen_port, None) => {
                let listen_port = listen_port.unwrap_or(1122);
                let listener =
                    TcpListener::bind(SocketAddr::new(arguments.listen_address, listen_port))
                        .context(format!(
                            "Error in Tcp bind {:?} {:?}",
                            arguments.listen_address, listen_port
                        ))?;
                let (client, addr) = listener.accept().context("failed to accept connection")?;
                info!("Client {:?}", addr);
                client.set_nodelay(true).context("Error in set_nodelay")?;
                Box::new(client)
            }
            (None, Some(unix_socket)) => {
                let client = UnixStream::connect(unix_socket)
                    .context(format!("Error in connect to unix socket {unix_socket:?}"))?;
                Box::new(client)
            }
            _ => {
                panic!("Choose between listen port and liten unix path");
            }
        };

    /* Connect to server */
    let mut server: Box<dyn ReadWrite> = if arguments.vsock {
        let port = arguments
            .server_port
            .parse::<u32>()
            .expect("Cannot parse port");
        let address = arguments
            .server_addr
            .parse::<u32>()
            .expect("Not a vsock address");
        let server = vsock::VsockStream::connect(&vsock::VsockAddr::new(address, port)).context(
            format!("Error in vsock server connection {address:?} {port:?}"),
        )?;
        info!("Connected to server");
        Box::new(server)
    } else {
        let port = arguments
            .server_port
            .parse::<u16>()
            .expect("Cannot parse port");
        let destination = format!("{}:{}", arguments.server_addr, port);
        let server = TcpStream::connect(&destination)
            .context(format!("Error in tcp server connection {destination:?}"))?;
        info!("Connected to server");
        server.set_nodelay(true).expect("set_nodelay call failed");
        Box::new(server)
    };

    /* Recv client version */
    let client_version: tunnel::Version =
        recv_client_msg_type!(&mut client, Version).context("Error in send client version")?;

    info!("Client version {:?}", client_version);
    if client_version.version != VERSION {
        return Err(anyhow!(
            "Version mismatch server: {:?} client: {:?}",
            VERSION,
            client_version.version
        ));
    }

    /* Forward version to server */
    send_client_msg_type!(&mut server, client_version, Version).context("Error in send Version")?;

    /* Recv server version */
    let server_version: tunnel::Version =
        recv_server_msg_type!(&mut server, Version).context("Error in recv server version")?;

    info!("Server version {:?}", server_version);
    if server_version.version != VERSION {
        return Err(anyhow!(
            "Version mismatch server: {:?} client: {:?}",
            server_version.version,
            VERSION,
        ));
    }

    /* Forward version to client */
    send_server_msg_type!(&mut client, server_version, Version).context("Error in send Version")?;

    /* recv server hello */
    let msg = recv_srv_msg_type!(&mut server, Hello)
        .context("Error in recv ServerHello")
        .map_err(|err| send_srv_err_event(&mut client, err))?;

    let server_size = match &msg.msg {
        Some(tunnel::server_hello::Msg::AdaptScreen(_)) => None,
        Some(tunnel::server_hello::Msg::Fullscreen(msg)) => {
            Some((msg.width as u16, msg.height as u16))
        }
        _ => {
            panic!("Unknown Server hello");
        }
    };

    /* Send server hello with image info & codec name */
    let server_hello = tunnel::ServerHello {
        codec_name,
        audio: arguments.audio,
        msg: msg.msg,
    };

    send_srv_msg_type!(&mut client, server_hello, Hello)
        .context("Error in send ServerHello")
        .map_err(|err| send_client_err_event(&mut server, err))?;

    let mut screen_size = if let Some((width, height)) = server_size {
        /* recv client hello with audio bool */
        let msg = recv_client_msg_type!(&mut client, Clienthellofullscreen)
            .context("Error in recv ClientHelloFullscreen")
            .map_err(|err| send_client_err_event(&mut server, err))?;
        debug!("{:?}", msg);
        send_client_msg_type!(&mut server, msg, Clienthellofullscreen)
            .context("Error in send ClientHelloFullscreen")
            .map_err(|err| send_srv_err_event(&mut client, err))?;
        (width, height)
    } else {
        /* recv client hello with audio bool */
        let msg = recv_client_msg_type!(&mut client, Clienthelloresolution)
            .context("Error in recv ClientHelloResolution")
            .map_err(|err| send_client_err_event(&mut server, err))?;

        debug!("{:?}", msg);
        let (width, height) = (msg.width as u16, msg.height as u16);
        send_client_msg_type!(&mut server, msg, Clienthelloresolution)
            .context("Error in recv ClientHelloResolution")
            .map_err(|err| send_srv_err_event(&mut client, err))?;
        (width, height)
    };

    let mut video_encoder = init_video_encoder(
        arguments.encoder.as_str(),
        config.ffmpeg_options(None),
        config.ffmpeg_options(Some(arguments.encoder.as_str())),
        &config.video.ffmpeg_options_cmd,
        (screen_size.0, screen_size.1),
    )?;

    // Do socket control
    let (control_sender, control_receiver) = channel();
    let control_path = config
        .video
        .control_path
        .as_ref()
        .map(|path| path.to_owned());
    if let Some(control_path) = control_path {
        info!("Listening on control path {:?}", control_path);
        thread::spawn(move || {
            let pid = std::process::id();
            let control_path = control_path.replace("%PID%", &format!("{pid}"));
            // Try to remove path first
            let _ = std::fs::remove_file(&control_path);
            let listener = std::os::unix::net::UnixListener::bind(&control_path)
                .unwrap_or_else(|_| panic!("Cannot bind {:?}", control_path));
            loop {
                let (_, addr) = listener.accept().expect("Error in UnixListener accept");
                info!("Client {:?}", addr);
                control_sender
                    .send("test".to_owned())
                    .expect("Cannot send control");
            }
        });
    }

    let mut count = 0;
    let mut sound_data = vec![];
    loop {
        // Test is we receiver control message
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

        /* Recv from server*/
        let msgs = recv_srv_msg_type!(&mut server, Msgssrv)
            .context("Error in recv MessagesSrv")
            .map_err(|err| send_srv_err_event(&mut client, err))?;

        let mut time_encode_video: Option<String> = None;
        let mut time_encode_sound: Option<String> = None;

        let mut events = vec![];
        for msg in msgs.msgs {
            match msg.msg {
                /* Disallow encoded image form server to client*/
                Some(tunnel::message_srv::Msg::ImgEncoded(_img)) => {
                    warn!("Filtering out encoded image");
                }
                Some(tunnel::message_srv::Msg::SoundEncoded(_sound)) => {
                    warn!("Server sent encoded sound");
                }

                Some(tunnel::message_srv::Msg::Stats(msg_stats)) => {
                    if let Some(ref proxy_stats) = time_encode_video {
                        let stats = msg_stats.stats + &format!(" proxy: {proxy_stats}");
                        trace!("server stats: {:?}", stats);
                        let msg = tunnel::message_srv::Msg::Stats(tunnel::EventStats { stats });
                        let msg = tunnel::MessageSrv { msg: Some(msg) };
                        events.push(msg);
                    }
                }
                event @ Some(tunnel::message_srv::Msg::Clipboard(_)) => {
                    if !arguments.disable_server_clipboard {
                        events.push(tunnel::MessageSrv { msg: event });
                    }
                }
                Some(tunnel::message_srv::Msg::Display(event)) => {
                    let (width, height) = (event.width, event.height);
                    debug!("New codec {}x{}", width, height);
                    let width = width & !1;
                    let height = height & !1;

                    video_encoder = video_encoder.change_resolution(width, height)?;
                    let msg = tunnel::EventDisplay { width, height };
                    let msg = tunnel::MessageSrv {
                        msg: Some(tunnel::message_srv::Msg::Display(msg)),
                    };
                    events.push(msg);
                }

                Some(tunnel::message_srv::Msg::ImgRaw(img)) => {
                    /* Encode raw image */
                    let time_encode_start = Instant::now();
                    let (data, width, height, bytes_per_line) = match &video_shared_mem {
                        Some(ref video_shared_mem) => match arguments.source_is_xwd {
                            true => {
                                let (data, _xwd_width, _xwd_height, bytes_per_line) =
                                    get_xwd_data(video_shared_mem)?;
                                (data, img.width, img.height, bytes_per_line)
                            }
                            false => {
                                let size = img.bytes_per_line as usize * img.height as usize;
                                let data = &video_shared_mem[..size];
                                (data, img.width, img.height, img.bytes_per_line)
                            }
                        },
                        _ => (&img.data[..], img.width, img.height, img.bytes_per_line),
                    };

                    if width > MAX_WINDOW_WIDTH
                        || height > MAX_WINDOW_HEIGHT
                        || bytes_per_line > MAX_BYTES_PER_LINE
                    {
                        panic!("Size too big {}x{} {}", width, height, bytes_per_line);
                    }

                    if width != screen_size.0 as u32 || height != screen_size.1 as u32 {
                        debug!("Resolution change {}x{}", width, height);
                        video_encoder = init_video_encoder(
                            &arguments.encoder,
                            config.ffmpeg_options(None),
                            config.ffmpeg_options(Some(arguments.encoder.as_str())),
                            &config.video.ffmpeg_options_cmd,
                            (width as u16, height as u16),
                        )
                        .context("Error in init_encoder")?;
                        screen_size.0 = width as u16;
                        screen_size.1 = height as u16;
                    }

                    let (encoded, timings) = video_encoder
                        .encode_image(data, width, height, bytes_per_line, count)
                        .unwrap();
                    let time_encode_stop = Instant::now();
                    let mut timings_str = String::new();
                    for timing in timings.times {
                        let time_str = format!("{:.1?}", timing.1);
                        write!(timings_str, "{} {:7}", timing.0, time_str)?;
                    }
                    time_encode_video = Some(format!(
                        "{:.1?} ({})",
                        time_encode_stop - time_encode_start,
                        timings_str
                    ));
                    count += 1;
                    let msg = tunnel::message_srv::Msg::ImgEncoded(tunnel::ImageEncoded {
                        data: encoded,
                        width: img.width,
                        height: img.height,
                    });
                    let msg_img = tunnel::MessageSrv { msg: Some(msg) };
                    events.push(msg_img);
                }
                Some(tunnel::message_srv::Msg::SoundRaw(sound_raw)) => {
                    /* Encode raw sound */
                    let mut rdr = Cursor::new(sound_raw.data);
                    let time_encode_start = Instant::now();
                    while let Ok(sample) = rdr.read_i16::<LittleEndian>() {
                        sound_data.push(sample);
                    }

                    if let Some(sound_event) = encode_sound(&mut sound_encoder, &mut sound_data) {
                        events.push(sound_event);
                    }
                    let time_encode_stop = Instant::now();
                    time_encode_sound =
                        Some(format!("{:.1?}", time_encode_stop - time_encode_start));
                }
                Some(msg) => {
                    /* Forward other events */
                    events.push(tunnel::MessageSrv { msg: Some(msg) });
                }
                _ => {}
            }
        }

        let msgs = tunnel::MessagesSrv { msgs: events };
        send_srv_msg_type!(&mut client, msgs, Msgssrv)
            .context("Error in send MessagesSrv")
            .map_err(|err| send_client_err_event(&mut server, err))?;

        /* Recv from client */
        let msgs = recv_client_msg_type!(&mut client, Msgsclient)
            .context("Error in recv MessagesClient")
            .map_err(|err| send_client_err_event(&mut server, err))?;

        let mut events = vec![];
        for msg in msgs.msgs {
            match msg.msg {
                event @ Some(tunnel::message_client::Msg::Clipboard(_)) => {
                    if !arguments.disable_client_clipboard {
                        events.push(tunnel::MessageClient { msg: event });
                    }
                }
                Some(msg) => {
                    /* Forward other events */
                    events.push(tunnel::MessageClient { msg: Some(msg) });
                }
                _ => {}
            }
        }
        let msgs = tunnel::MessagesClient { msgs: events };

        send_client_msg_type!(&mut server, msgs, Msgsclient)
            .context("Error in send MessagesClient")
            .map_err(|err| send_srv_err_event(&mut client, err))?;

        let time_encode_video = match time_encode_video {
            None => "-".to_owned(),
            Some(time_encode_video) => time_encode_video.to_owned(),
        };
        let time_encode_sound = match time_encode_sound {
            None => "-".to_owned(),
            Some(time_encode_sound) => time_encode_sound.to_owned(),
        };
        debug!(
            "Loop: encode: video: {:7} sound: {:7}",
            time_encode_video, time_encode_sound
        );
    }
}
