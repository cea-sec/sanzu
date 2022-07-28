use anyhow::{Context, Result};
extern crate libc;
use std::{collections::HashMap, fmt::Write as _, net::TcpStream, time::Instant};

use std::{
    convert::TryInto,
    io,
    io::Read,
    io::Write,
    process::{ChildStdin, ChildStdout, Command, Stdio},
    thread,
};

use sanzu_common::{
    proto::{recv_server_msg_or_error, send_client_err_event, VERSION},
    tls_helper::make_client_config,
    tunnel, ReadWrite, Tunnel,
};

#[cfg(feature = "kerberos")]
use sanzu_common::auth_kerberos::do_kerberos_server_auth;

use crate::{
    client_graphics::*,
    client_utils::Area,
    config::ConfigClient,
    osd::{draw_text, TestDisplay},
    //proto::{Tunnel, ReadWrite},
    sound::SoundDecoder,
    utils::{
        ArgumentsClient, MAX_BYTES_PER_LINE, MAX_CURSOR_HEIGHT, MAX_CURSOR_WIDTH,
        MAX_WINDOW_HEIGHT, MAX_WINDOW_WIDTH,
    },
    video_decoder::init_video_codec,
};

const SHELL_PATH: &str = "/bin/sh";

struct StreamPipes {
    pipe_in: ChildStdout,
    pipe_out: ChildStdin,
}

impl Read for StreamPipes {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.pipe_in.read(buf)
    }
}

impl Write for StreamPipes {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pipe_out.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.pipe_out.flush()
    }
}

fn check_img_size(width: u32, height: u32) -> Result<(u32, u32)> {
    if width > MAX_WINDOW_WIDTH || height > MAX_WINDOW_HEIGHT {
        Err(anyhow!("Err img too big {}x{}", width, height))
    } else {
        Ok((width, height))
    }
}

fn check_cusor_size(width: u32, height: u32, xhot: u32, yhot: u32) -> Result<(u32, u32, u32, u32)> {
    if width > MAX_CURSOR_WIDTH
        || height > MAX_CURSOR_HEIGHT
        || xhot > MAX_CURSOR_WIDTH
        || yhot > MAX_CURSOR_HEIGHT
    {
        Err(anyhow!(
            "Err cursor too big {}x{} ({} {})",
            width,
            height,
            xhot,
            yhot
        ))
    } else {
        Ok((width, height, xhot, yhot))
    }
}

pub trait ClientInterface {
    fn pam_echo(&mut self, echo: String) -> Result<String>;

    fn pam_blind(&mut self, blind: String) -> Result<String>;

    fn pam_info(&mut self, info: String) -> Result<()>;

    fn pam_error(&mut self, error: String) -> Result<()>;

    fn pam_end(&mut self, end: bool) -> Result<()>;

    fn client_exit(&mut self, status: &Result<()>);
}

#[derive(Default)]
pub struct StdioClientInterface {}

impl ClientInterface for StdioClientInterface {
    fn pam_echo(&mut self, echo: String) -> Result<String> {
        println!("{}", echo);
        let mut user = String::new();
        let stdin = std::io::stdin();
        std::io::stdout().flush().unwrap();
        stdin.read_line(&mut user).context("Error in read login")?;

        // We use trim here assuming it's suitable for logins
        // which should not end with a whitespace
        let len = user.trim().len();
        user.truncate(len);

        Ok(user)
    }

    fn pam_blind(&mut self, blind: String) -> Result<String> {
        rpassword::prompt_password(blind).context("Error in read password")
    }

    fn pam_info(&mut self, info: String) -> Result<()> {
        println!("{}", info);
        Ok(())
    }

    fn pam_error(&mut self, error: String) -> Result<()> {
        println!("{}", error);
        Ok(())
    }

    fn pam_end(&mut self, end: bool) -> Result<()> {
        match end {
            true => {
                info!("Pam end ok");
            }
            false => {
                info!("Pam end err");
            }
        }
        Ok(())
    }

    fn client_exit(&mut self, _status: &Result<()>) {}
}

/// Client main loop
///
/// The loop is composed of the following actions:
/// - poll client graphics events (mouse move, key down/up, clipboard, ...)
/// - send those events to the server
/// - receive events from the server
/// - decode and handle those events (image decoding, sound, notifications, clipboard, ...)
/// - image update if necessary
///

pub fn run(
    client_config: &ConfigClient,
    arguments: &ArgumentsClient,
    mut client_interface: impl ClientInterface,
) -> Result<()> {
    let res = do_run(client_config, arguments, &mut client_interface);
    client_interface.client_exit(&res);
    res
}

pub fn do_run(
    client_config: &ConfigClient,
    arguments: &ArgumentsClient,
    client_interface: &mut impl ClientInterface,
) -> Result<()> {
    let mut sound_obj = if arguments.audio {
        Some(
            SoundDecoder::new(
                "default",
                arguments.audio_sample_rate,
                arguments.audio_buffer_ms,
            )
            .context("Error in new SoundDecoder")?,
        )
    } else {
        None
    };

    let (audio, audio_sample_rate) = match &sound_obj {
        Some(ref sound_obj) => (true, sound_obj.sample_rate),
        None => (false, 0),
    };

    let mut socket: Box<dyn ReadWrite> = match &arguments.proxycommand {
        None => {
            let destination = format!("{}:{}", arguments.address, arguments.port);
            let socket = TcpStream::connect(&destination)
                .context(format!("Error in server connection {:?}", destination))?;
            socket.set_nodelay(true).context("Error in set_nodelay")?;
            Box::new(socket)
        }
        Some(commandline) => {
            /* Launch proxy command*/
            let mut child = Command::new(SHELL_PATH)
                .arg("-c")
                .arg(&commandline)
                .stdout(Stdio::piped())
                .stdin(Stdio::piped())
                .spawn()
                .context("Error in launch proxycommand")?;
            info!("Proxycommand {:?}", child);

            let pipe_child_in = child.stdin.take().context("Error in get stdin")?;
            let pipe_child_out = child.stdout.take().context("Error in get stdout")?;
            let stream = StreamPipes {
                pipe_in: pipe_child_out,
                pipe_out: pipe_child_in,
            };

            thread::spawn(move || {
                debug!("Wait proxycommand");
                child.wait().expect("Error in wait proxycommand");
                debug!("End proxycommand");
            });
            Box::new(stream)
        }
    };
    debug!("Connected");

    let tls_server_name_ok = arguments.tls_server_name.unwrap_or("no_server_name");
    let server_name = tls_server_name_ok
        .try_into()
        .map_err(|err| anyhow!("Err {:?}", err))
        .context("Error in dns server tls name")?;
    let config = make_client_config(
        arguments.tls_ca,
        arguments.client_cert,
        arguments.client_key,
    )
    .context("Error in make client tls config")?;
    let mut conn = rustls::ClientConnection::new(config, server_name)
        .context("Error in new ClientConnection")?;
    let mut tls = rustls::Stream::new(&mut conn, &mut socket);

    let server: &mut dyn ReadWrite = if arguments.tls_server_name.is_some() {
        &mut tls
    } else {
        &mut socket
    };

    // Send client version
    let client_version = tunnel::Version {
        version: VERSION.to_owned(),
    };
    send_client_msg_type!(server, client_version, Version).context("Error in send Version")?;

    /* Recv client version */
    let server_version: tunnel::Version =
        recv_server_msg_type!(server, Version).context("Error in send server version")?;

    info!("Server version {:?}", server_version);
    if server_version.version != VERSION {
        return Err(anyhow!(
            "Version mismatch server: {:?} client: {:?}",
            server_version.version,
            VERSION,
        ));
    }

    #[cfg(feature = "kerberos")]
    if let Some(cname) = arguments.server_cname {
        do_kerberos_server_auth(cname, server)
            .context("Error in perform_auth")
            .map_err(|err| send_client_err_event(server, err))?
    }
    #[cfg(not(feature = "kerberos"))]
    debug!("Skipping kerberos auth");

    if arguments.login {
        if arguments.tls_server_name.is_none() {
            println!("WARNING: no tls, password will be sent in clear text");
        }
        loop {
            let msg = recv_server_msg_type!(server, Pamconversation)
                .context("Error in recv PamConversation")?;

            match msg.msg {
                Some(tunnel::pam_conversation::Msg::Echo(echo)) => {
                    let user = client_interface
                        .pam_echo(echo)
                        .map_err(|err| send_client_err_event(server, err))?;
                    let client_user = tunnel::EventPamUser { user };
                    send_client_msg_type!(server, client_user, Pamuser)
                        .context("Error in send EventPamUser")?;
                }
                Some(tunnel::pam_conversation::Msg::Blind(blind)) => {
                    let password = client_interface
                        .pam_blind(blind)
                        .map_err(|err| send_client_err_event(server, err))?;
                    let client_pwd = tunnel::EventPamPwd { password };
                    send_client_msg_type!(server, client_pwd, Pampwd)
                        .context("Error in send EventPamPwd")?;
                }

                Some(tunnel::pam_conversation::Msg::Info(info)) => {
                    client_interface.pam_info(info)?;
                }
                Some(tunnel::pam_conversation::Msg::Error(err)) => {
                    client_interface.pam_error(err)?;
                }
                Some(tunnel::pam_conversation::Msg::End(end)) => {
                    client_interface.pam_end(end)?;
                    break;
                }
                None => {
                    return Err(anyhow!("Err on Pam conversation"));
                }
            }
        }
    }

    /* Receive image info & codec name */
    let msg = recv_server_msg_type!(server, Hello).context("Error in recv ServerHello")?;

    info!("{:?}", msg);
    let codec_name = match arguments.decoder_name {
        Some(decoder_name) => decoder_name.to_owned(),
        None => msg.codec_name.to_owned(),
    };
    let (seamless, server_size) = match msg.msg {
        Some(tunnel::server_hello::Msg::AdaptScreen(adapt_screen)) => (adapt_screen.seamless, None),
        Some(tunnel::server_hello::Msg::Fullscreen(msg)) => {
            (false, Some((msg.width as u16, msg.height as u16)))
        }
        _ => {
            panic!("Unknown Server hello");
        }
    };

    #[cfg(unix)]
    let mut client = init_x11rb(arguments, seamless, server_size)
        .context("Error in init_x11rb")
        .map_err(|err| send_client_err_event(server, err))?;
    #[cfg(windows)]
    let mut client = init_wind3d(arguments, seamless, server_size)
        .context("Error in init_wind3d")
        .map_err(|err| send_client_err_event(server, err))?;

    /* Send hello with audio bool */
    let (mut img_width, mut img_height) = match server_size {
        Some((width, height)) => {
            let client_hello = tunnel::ClientHelloFullscreen {
                audio,
                audio_sample_rate,
            };
            send_client_msg_type!(server, client_hello, Clienthellofullscreen)
                .context("Error in send ClientHelloFullscreen")?;
            (width, height)
        }
        None => {
            let (width, height) = client.size();
            let width_even = width as u32 & !1;
            let height_event = height as u32 & !1;
            let client_hello = tunnel::ClientHelloResolution {
                audio,
                audio_sample_rate,
                width: width_even,
                height: height_event,
            };
            send_client_msg_type!(server, client_hello, Clienthelloresolution)
                .context("Error in send ClientHelloResolution")?;
            (width, height)
        }
    };

    let mut decoder =
        init_video_codec(client_config.ffmpeg_options(Some(&codec_name)), &codec_name)
            .context("Cannot init video decoder")
            .map_err(|err| send_client_err_event(server, err))?;

    if let Some(ref mut sound_obj) = sound_obj {
        sound_obj
            .start()
            .context("Error in sound start")
            .map_err(|err| send_client_err_event(server, err))?;
    }

    let mut stats = "".to_owned();
    let mut img_bytes_per_line = None;
    loop {
        let mut areas = HashMap::new();
        let time_start = Instant::now();

        let msgs = client.poll_events().context("Error in poll_events")?;

        let time_events = Instant::now();

        send_client_msg_type!(server, msgs, Msgsclient).context("Error in send client events")?;

        let time_send = Instant::now();

        /* Decode encoded img */
        let msg: tunnel::MessagesSrv =
            recv_server_msg_type!(server, Msgssrv).context("Error in recv MessagesSrv")?;

        let time_recv = Instant::now();

        let mut img_todo = None;

        for msg in msg.msgs {
            match msg.msg {
                Some(tunnel::message_srv::Msg::ImgEncoded(img)) => {
                    let (width, height) = check_img_size(img.width, img.height)
                        .map_err(|err| send_client_err_event(server, err))?;
                    img_todo = Some((img.data, width, height));
                }
                Some(tunnel::message_srv::Msg::ImgRaw(img)) => {
                    let (width, height) = check_img_size(img.width, img.height)
                        .map_err(|err| send_client_err_event(server, err))?;
                    img_todo = Some((img.data, width, height));
                    if img.bytes_per_line > MAX_BYTES_PER_LINE {
                        return Err(anyhow!("Bytes per lines too big"));
                    }
                    img_bytes_per_line = Some(img.bytes_per_line as u16);
                }
                Some(tunnel::message_srv::Msg::SoundEncoded(sound)) => {
                    if let Some(ref mut sound_obj) = sound_obj {
                        for pkt in sound.data {
                            sound_obj.push(pkt);
                        }
                    }
                }
                Some(tunnel::message_srv::Msg::Clipboard(clipboard)) => {
                    info!("Clipboard retrieved from server");
                    if client.set_clipboard(&clipboard.data).is_err() {
                        error!("Cannot set clipboard");
                    }
                }
                Some(tunnel::message_srv::Msg::Cursor(cursor)) => {
                    if let Err(err) =
                        check_cusor_size(cursor.width, cursor.height, cursor.xhot, cursor.yhot)
                            .map_err(|err| err.context("Cursor size error"))
                            .map(|(width, height, xhot, yhot)| {
                                client.set_cursor(
                                    &cursor.data,
                                    (width, height),
                                    (xhot as u16, yhot as u16),
                                )
                            })
                            .map_err(|err| err.context("Set cursor error"))
                    {
                        error!("Updt cursor error");
                        err.chain().for_each(|cause| error!(" - due to {}", cause));
                    }
                }
                Some(tunnel::message_srv::Msg::AreaUpdt(area_updt)) => {
                    trace!("new updt: {:?}", area_updt);
                    let area = Area {
                        id: area_updt.id as usize,
                        size: (area_updt.width as u16, area_updt.height as u16),
                        position: (area_updt.x as i16, area_updt.y as i16),
                        mapped: area_updt.mapped,
                    };
                    areas.insert(area_updt.id as usize, area);
                }
                Some(tunnel::message_srv::Msg::Printfile(printfile)) => {
                    trace!("printfile: {:?}", printfile);
                    #[cfg(feature = "printfile")]
                    {
                        info!("printfile: {:?}", printfile);
                        if let Err(err) =
                            client.printfile(&printfile.path).context("Error in print")
                        {
                            err.chain().for_each(|cause| error!(" - due to {}", cause));
                        }
                    }
                }
                Some(tunnel::message_srv::Msg::Notifications(notifications)) => {
                    trace!("notifications: {:?}", notifications);
                    #[cfg(feature = "notify")]
                    {
                        let mut notification_title = None;
                        let mut notification_icon = None;
                        let mut strings = vec![];
                        if !notifications.notifications.is_empty() {
                            for notification in notifications.notifications {
                                match notification.msg {
                                    Some(tunnel::notification::Msg::Title(string)) => {
                                        notification_title = Some(string);
                                    }
                                    Some(tunnel::notification::Msg::Message(string)) => {
                                        strings.push(string);
                                    }
                                    Some(tunnel::notification::Msg::Icon(icon)) => {
                                        if let Ok(icon) = notify_rust::Image::from_rgba(
                                            icon.width as i32,
                                            icon.height as i32,
                                            icon.data,
                                        ) {
                                            notification_icon = Some(icon);
                                        } else {
                                            error!("Cannot create image");
                                        }
                                    }
                                    _ => {}
                                }
                            }

                            let message = strings.join("\n");
                            let mut notification = notify_rust::Notification::new();
                            if let Some(title) = notification_title {
                                notification.summary = title;
                            }
                            if let Some(icon) = notification_icon {
                                notification
                                    .hints
                                    .insert(notify_rust::Hint::ImageData(icon));
                            }
                            notification.body = message;
                            if notification.show().is_err() {
                                error!("Cannot notify");
                            }
                        }
                    }
                }
                Some(tunnel::message_srv::Msg::Stats(msg_stats)) => {
                    trace!("server stats: {:?}", stats);
                    stats = msg_stats.stats
                }
                _ => {}
            };
        }

        client.update(&areas).context("Error in update")?;

        let time_decode_msgs = Instant::now();
        let mut time_decode = None;

        if let Some((img_data, new_img_width, new_img_height)) = img_todo {
            if img_width != new_img_width as u16 || img_height != new_img_height as u16 {
                info!("New resolution {}x{}", new_img_width, new_img_height);
                decoder = decoder.reload().context(format!(
                    "Cannot reload decode with size {}x{}",
                    new_img_width, new_img_height
                ))?;
                img_width = new_img_width as u16;
                img_height = new_img_height as u16;
                info!("New codec ok");
            }

            if let (Some(_img_updated), Some(mut timings)) =
                decoder.decode_img(&img_data, img_width, img_height, img_bytes_per_line)
            {
                let time_start = Instant::now();
                if let Some(data_rgba) = decoder.data_rgba().as_mut() {
                    if client.display_stats() {
                        let mut display = TestDisplay {
                            width: img_width as u32,
                            height: img_height as u32,
                            buffer: data_rgba,
                        };
                        let stats = stats.replace('µ', "u");
                        draw_text(&mut display, &stats, 0, img_height as i32 - 50);
                    }

                    client
                        .set_img(
                            &data_rgba[0..img_width as usize * img_height as usize * 4],
                            (img_width as u32, img_height as u32),
                        )
                        .context("Error in set_img")?;
                }
                let time_set_img = Instant::now();
                timings.times.push(("set", time_set_img - time_start));
                time_decode = Some(timings);
            }
        }

        let time_stop = Instant::now();

        let mut timings_str = String::new();
        let times_img = if let Some(timings) = time_decode {
            for timing in timings.times {
                let time_str = format!("{:.1?}", timing.1);
                write!(timings_str, "{} {:8}", timing.0, time_str)?;
            }
            timings_str
        } else {
            "  -  ".to_owned()
        };

        debug!(
            "Total: {:>7} events: {:>7} send: {:>7} recv: {:>7} decode msg: {:>7} ({:14})",
            &format!("{:.1?}", time_stop - time_start),
            &format!("{:.1?}", time_events - time_start),
            &format!("{:.1?}", time_send - time_events),
            &format!("{:.1?}", time_recv - time_send),
            &format!("{:.1?}", time_decode_msgs - time_recv),
            &times_img,
        );
    }
}
