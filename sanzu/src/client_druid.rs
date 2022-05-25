use crate::proto::tunnel;

use druid::{widget::prelude::*, CursorDesc, ImageBuf, Point, Rect, Screen};
use druid_shell::{
    Application, Code as DruidKeyCode, KeyEvent, MouseButton, MouseEvent, Region, WinHandler,
    WindowBuilder, WindowHandle,
};
use err_derive::Error;
use piet_common::{kurbo::Size, Piet};
use std::{
    any::Any,
    collections::HashMap,
    sync::{mpsc::channel, mpsc::sync_channel},
    thread,
};

const SCALE_X: f64 = 1.0414;
const SCALE_Y: f64 = 1.0412;

#[derive(Debug, Error)]
pub enum ClientGraphicError {
    #[error(display = "Error")]
    Error,
    #[error(display = "IO: {}", _0)]
    IO(#[source] std::io::Error),
    #[error(display = "x11rb: {}", _0)]
    ConnectError(#[error(source, from)] x11rb::errors::ConnectError),
    #[error(display = "x11rb: {}", _0)]
    ConnectionError(#[error(source, from)] x11rb::errors::ConnectionError),
    #[error(display = "x11rb: {}", _0)]
    ReplyError(#[error(source, from)] x11rb::errors::ReplyError),
    #[error(display = "x11rb: {}", _0)]
    ReplyOrIdError(#[error(source, from)] x11rb::errors::ReplyOrIdError),
    #[error(display = "Sound: {}", _0)]
    SoundError(#[error(source, from)] crate::sound::SoundError),
    #[error(display = "PlayStreamError: {}", _0)]
    PlayStreamError(#[error(source, from)] cpal::PlayStreamError),
    #[error(display = "Send: {}", _0)]
    FrameSendError(#[source] std::sync::mpsc::SendError<(Vec<u8>, u32, u32)>),
    #[error(display = "Send: {}", _0)]
    CursorSendError(#[source] std::sync::mpsc::SendError<(Vec<u8>, u32, u32, i32, i32)>),
}

impl From<Box<dyn std::error::Error>> for ClientGraphicError {
    fn from(_: Box<dyn std::error::Error>) -> ClientGraphicError {
        ClientGraphicError::Error
    }
}

type Result<T> = std::result::Result<T, ClientGraphicError>;

pub struct VideoApp {
    pub handle: WindowHandle,
    pub frame_receiver: std::sync::mpsc::Receiver<(Vec<u8>, u32, u32)>,
    pub event_sender: std::sync::mpsc::Sender<tunnel::MessageClient>,
    pub cursor_receiver: std::sync::mpsc::Receiver<(Vec<u8>, u32, u32, i32, i32)>,
    pub keys_state: Vec<bool>,
    pub width: u32,
    pub height: u32,
    pub size: Size,
}

pub struct ClientInfo {
    pub frame_sender: std::sync::mpsc::SyncSender<(Vec<u8>, u32, u32)>,
    pub cursor_sender: std::sync::mpsc::Sender<(Vec<u8>, u32, u32, i32, i32)>,
    pub event_receiver: std::sync::mpsc::Receiver<tunnel::MessageClient>,
    pub width: u16,
    pub height: u16,
}

impl ClientInfo {
    pub fn build(
        server_size: Option<(u16, u16)>,
    ) -> (
        ClientInfo,
        std::sync::mpsc::Receiver<(Vec<u8>, u32, u32)>,
        std::sync::mpsc::Sender<tunnel::MessageClient>,
        std::sync::mpsc::Receiver<(Vec<u8>, u32, u32, i32, i32)>,
    ) {
        let (frame_sender, frame_receiver) = sync_channel(1);
        let (event_sender, event_receiver) = channel();

        let (cursor_sender, cursor_receiver) = channel();

        let (width, height) = match server_size {
            Some((width, height)) => (width, height),
            None => {
                let monitors = Screen::get_monitors();
                let monitor = monitors.get(0).expect("Cannot get monitor 0");
                let monitor_rect = monitor.virtual_work_rect();
                let width = (monitor_rect.x1 - monitor_rect.x0) as u16;
                let height = (monitor_rect.y1 - monitor_rect.y0) as u16;
                (width, height)
            }
        };

        let video_client = ClientInfo {
            frame_sender,
            cursor_sender,
            event_receiver,
            width,
            height,
        };

        (video_client, frame_receiver, event_sender, cursor_receiver)
    }
}

impl VideoApp {
    pub fn new(
        frame_receiver: std::sync::mpsc::Receiver<(Vec<u8>, u32, u32)>,
        event_sender: std::sync::mpsc::Sender<tunnel::MessageClient>,
        cursor_receiver: std::sync::mpsc::Receiver<(Vec<u8>, u32, u32, i32, i32)>,
        width: u32,
        height: u32,
    ) -> VideoApp {
        VideoApp {
            size: Size::ZERO,
            handle: Default::default(),
            keys_state: vec![false; 0x100],
            frame_receiver,
            event_sender,
            cursor_receiver,
            width,
            height,
        }
    }
}

#[derive(Debug)]
pub struct Area {
    pub id: usize,
    pub size: (u16, u16),
    pub position: (i16, i16),
}

pub fn set_clipboard(_client_info: &mut ClientInfo, data: String) -> Result<()> {
    let mut clipboard = Application::global().clipboard();
    clipboard.put_string(data);
    Ok(())
}

pub fn poll_events(client_info: &mut ClientInfo) -> Result<tunnel::MessagesClient> {
    let mut events = vec![];
    let mut last_move = None;
    while let Ok(event) = client_info.event_receiver.try_recv() {
        match event {
            event
            @
            tunnel::MessageClient {
                msg: Some(tunnel::message_client::Msg::Move { .. }),
            } => last_move = Some(event),
            _ => events.push(event),
        }
    }

    if let Some(event) = last_move {
        events.push(event)
    }

    Ok(tunnel::MessagesClient { msgs: events })
}

/* We want hardware keycode! */
pub fn code_to_hardware_keycode(keycode: DruidKeyCode) -> Option<u16> {
    let hw_keycode = match keycode {
        DruidKeyCode::Escape => 0x0009,
        DruidKeyCode::Digit1 => 0x000A,
        DruidKeyCode::Digit2 => 0x000B,
        DruidKeyCode::Digit3 => 0x000C,
        DruidKeyCode::Digit4 => 0x000D,
        DruidKeyCode::Digit5 => 0x000E,
        DruidKeyCode::Digit6 => 0x000F,
        DruidKeyCode::Digit7 => 0x0010,
        DruidKeyCode::Digit8 => 0x0011,
        DruidKeyCode::Digit9 => 0x0012,
        DruidKeyCode::Digit0 => 0x0013,
        DruidKeyCode::Minus => 0x0014,
        DruidKeyCode::Equal => 0x0015,
        DruidKeyCode::Backspace => 0x0016,
        DruidKeyCode::Tab => 0x0017,
        DruidKeyCode::KeyQ => 0x0018,
        DruidKeyCode::KeyW => 0x0019,
        DruidKeyCode::KeyE => 0x001A,
        DruidKeyCode::KeyR => 0x001B,
        DruidKeyCode::KeyT => 0x001C,
        DruidKeyCode::KeyY => 0x001D,
        DruidKeyCode::KeyU => 0x001E,
        DruidKeyCode::KeyI => 0x001F,
        DruidKeyCode::KeyO => 0x0020,
        DruidKeyCode::KeyP => 0x0021,
        DruidKeyCode::BracketLeft => 0x0022,
        DruidKeyCode::BracketRight => 0x0023,
        DruidKeyCode::Enter => 0x0024,
        DruidKeyCode::ControlLeft => 0x0025,
        DruidKeyCode::KeyA => 0x0026,
        DruidKeyCode::KeyS => 0x0027,
        DruidKeyCode::KeyD => 0x0028,
        DruidKeyCode::KeyF => 0x0029,
        DruidKeyCode::KeyG => 0x002A,
        DruidKeyCode::KeyH => 0x002B,
        DruidKeyCode::KeyJ => 0x002C,
        DruidKeyCode::KeyK => 0x002D,
        DruidKeyCode::KeyL => 0x002E,
        DruidKeyCode::Semicolon => 0x002F,
        DruidKeyCode::Quote => 0x0030,
        DruidKeyCode::Backquote => 0x0031,
        DruidKeyCode::ShiftLeft => 0x0032,
        DruidKeyCode::Backslash => 0x0033,
        DruidKeyCode::KeyZ => 0x0034,
        DruidKeyCode::KeyX => 0x0035,
        DruidKeyCode::KeyC => 0x0036,
        DruidKeyCode::KeyV => 0x0037,
        DruidKeyCode::KeyB => 0x0038,
        DruidKeyCode::KeyN => 0x0039,
        DruidKeyCode::KeyM => 0x003A,
        DruidKeyCode::Comma => 0x003B,
        DruidKeyCode::Period => 0x003C,
        DruidKeyCode::Slash => 0x003D,
        DruidKeyCode::ShiftRight => 0x003E,
        DruidKeyCode::NumpadMultiply => 0x003F,
        DruidKeyCode::AltLeft => 0x0040,
        DruidKeyCode::Space => 0x0041,
        DruidKeyCode::CapsLock => 0x0042,
        DruidKeyCode::F1 => 0x0043,
        DruidKeyCode::F2 => 0x0044,
        DruidKeyCode::F3 => 0x0045,
        DruidKeyCode::F4 => 0x0046,
        DruidKeyCode::F5 => 0x0047,
        DruidKeyCode::F6 => 0x0048,
        DruidKeyCode::F7 => 0x0049,
        DruidKeyCode::F8 => 0x004A,
        DruidKeyCode::F9 => 0x004B,
        DruidKeyCode::F10 => 0x004C,
        DruidKeyCode::NumLock => 0x004D,
        DruidKeyCode::ScrollLock => 0x004E,
        DruidKeyCode::Numpad7 => 0x004F,
        DruidKeyCode::Numpad8 => 0x0050,
        DruidKeyCode::Numpad9 => 0x0051,
        DruidKeyCode::NumpadSubtract => 0x0052,
        DruidKeyCode::Numpad4 => 0x0053,
        DruidKeyCode::Numpad5 => 0x0054,
        DruidKeyCode::Numpad6 => 0x0055,
        DruidKeyCode::NumpadAdd => 0x0056,
        DruidKeyCode::Numpad1 => 0x0057,
        DruidKeyCode::Numpad2 => 0x0058,
        DruidKeyCode::Numpad3 => 0x0059,
        DruidKeyCode::Numpad0 => 0x005A,
        DruidKeyCode::NumpadDecimal => 0x005B,
        DruidKeyCode::IntlBackslash => 0x005E,
        DruidKeyCode::F11 => 0x005F,
        DruidKeyCode::F12 => 0x0060,
        DruidKeyCode::IntlRo => 0x0061,
        DruidKeyCode::Convert => 0x0064,
        DruidKeyCode::KanaMode => 0x0065,
        DruidKeyCode::NonConvert => 0x0066,
        DruidKeyCode::NumpadEnter => 0x0068,
        DruidKeyCode::ControlRight => 0x0069,
        DruidKeyCode::NumpadDivide => 0x006A,
        DruidKeyCode::PrintScreen => 0x006B,
        DruidKeyCode::AltRight => 0x006C,
        DruidKeyCode::Home => 0x006E,
        DruidKeyCode::ArrowUp => 0x006F,
        DruidKeyCode::PageUp => 0x0070,
        DruidKeyCode::ArrowLeft => 0x0071,
        DruidKeyCode::ArrowRight => 0x0072,
        DruidKeyCode::End => 0x0073,
        DruidKeyCode::ArrowDown => 0x0074,
        DruidKeyCode::PageDown => 0x0075,
        DruidKeyCode::Insert => 0x0076,
        DruidKeyCode::Delete => 0x0077,
        DruidKeyCode::AudioVolumeMute => 0x0079,
        DruidKeyCode::AudioVolumeDown => 0x007A,
        DruidKeyCode::AudioVolumeUp => 0x007B,
        DruidKeyCode::NumpadEqual => 0x007D,
        DruidKeyCode::Pause => 0x007F,
        DruidKeyCode::NumpadComma => 0x0081,
        DruidKeyCode::Lang1 => 0x0082,
        DruidKeyCode::Lang2 => 0x0083,
        DruidKeyCode::IntlYen => 0x0084,
        DruidKeyCode::MetaLeft => 0x0085,
        DruidKeyCode::MetaRight => 0x0086,
        DruidKeyCode::ContextMenu => 0x0087,
        DruidKeyCode::BrowserStop => 0x0088,
        DruidKeyCode::Again => 0x0089,
        DruidKeyCode::Props => 0x008A,
        DruidKeyCode::Undo => 0x008B,
        DruidKeyCode::Select => 0x008C,
        DruidKeyCode::Copy => 0x008D,
        DruidKeyCode::Open => 0x008E,
        DruidKeyCode::Paste => 0x008F,
        DruidKeyCode::Find => 0x0090,
        DruidKeyCode::Cut => 0x0091,
        DruidKeyCode::Help => 0x0092,
        DruidKeyCode::LaunchApp2 => 0x0094,
        DruidKeyCode::WakeUp => 0x0097,
        DruidKeyCode::LaunchApp1 => 0x0098,
        // key to right of volume controls on T430s produces 0x9C
        // but no documentation of what it should map to :/
        DruidKeyCode::LaunchMail => 0x00A3,
        DruidKeyCode::BrowserFavorites => 0x00A4,
        DruidKeyCode::BrowserBack => 0x00A6,
        DruidKeyCode::BrowserForward => 0x00A7,
        DruidKeyCode::Eject => 0x00A9,
        DruidKeyCode::MediaTrackNext => 0x00AB,
        DruidKeyCode::MediaPlayPause => 0x00AC,
        DruidKeyCode::MediaTrackPrevious => 0x00AD,
        DruidKeyCode::MediaStop => 0x00AE,
        DruidKeyCode::MediaSelect => 0x00B3,
        DruidKeyCode::BrowserHome => 0x00B4,
        DruidKeyCode::BrowserRefresh => 0x00B5,
        DruidKeyCode::BrowserSearch => 0x00E1,
        DruidKeyCode::Unidentified => return None,
        _ => return None,
    };
    Some(hw_keycode)
}

impl WinHandler for VideoApp {
    fn connect(&mut self, handle: &WindowHandle) {
        self.handle = handle.clone();
    }

    fn prepare_paint(&mut self) {
        self.handle.invalidate();
    }

    fn paint(&mut self, piet: &mut Piet, _: &Region) {
        while let Ok((cursor_data, width, height, xhot, yhot)) = self.cursor_receiver.try_recv() {
            let cursor_img = ImageBuf::from_raw(
                cursor_data,
                piet_common::ImageFormat::RgbaSeparate,
                width as usize,
                height as usize,
            );
            let hot = Point {
                x: xhot as f64,
                y: yhot as f64,
            };
            let cursor_desc = CursorDesc::new(cursor_img, hot);
            let cursor = self
                .handle
                .make_cursor(&cursor_desc)
                .expect("Cannot build cursor");
            self.handle.set_cursor(&cursor);
        }

        while let Ok((data, width, height)) = self.frame_receiver.try_recv() {
            // put frame
            let rect = self.size.to_rect();
            let (x1, y1) = (rect.x1, rect.y1);

            let imgx = width;
            let imgy = height;
            let mut imgx_dst = imgx as f64;
            let mut imgy_dst = imgy as f64;

            if imgx_dst > x1 {
                imgx_dst = x1;
            }

            if imgy_dst > y1 {
                imgy_dst = y1;
            }

            let img = piet
                .make_image(
                    imgx as usize,
                    imgy as usize,
                    &data,
                    piet_common::ImageFormat::RgbaSeparate,
                )
                .unwrap();
            // TODO XXX: how not to strech pixels?
            let rect_src = Rect::new(0.0, 0.0, imgx_dst as f64, imgy_dst as f64);
            let rect_dst = Rect::new(0.0, 0.0, imgx_dst / SCALE_X, imgy_dst / SCALE_Y);

            piet.draw_image_area(
                &img,
                rect_src,
                rect_dst,
                piet_common::InterpolationMode::NearestNeighbor,
            );
        }

        self.handle.request_anim_frame();
    }

    fn command(&mut self, id: u32) {
        match id {
            0x100 => self.handle.close(),
            _ => warn!("unexpected id {}", id),
        }
    }

    fn key_down(&mut self, event: KeyEvent) -> bool {
        debug!(
            "keydown: {:?} {:?}",
            code_to_hardware_keycode(event.code),
            event
        );
        if let Some(hw_keycode) = code_to_hardware_keycode(event.code) {
            self.keys_state[hw_keycode as usize] = true;
            let eventkey = tunnel::EventKey {
                keycode: hw_keycode as u32,
                updown: true,
            };
            let msg_event = tunnel::MessageClient {
                msg: Some(tunnel::message_client::Msg::Key(eventkey)),
            };
            self.event_sender.send(msg_event).expect("Cannot send");
        } else {
            error!("Bad key {:?}!", event.code);
        }
        false
    }

    fn key_up(&mut self, event: KeyEvent) {
        debug!(
            "keyup: {:?} {:?}",
            code_to_hardware_keycode(event.code),
            event
        );
        if let Some(hw_keycode) = code_to_hardware_keycode(event.code) {
            self.keys_state[hw_keycode as usize] = true;
            let eventkey = tunnel::EventKey {
                keycode: hw_keycode as u32,
                updown: false,
            };
            let msg_event = tunnel::MessageClient {
                msg: Some(tunnel::message_client::Msg::Key(eventkey)),
            };
            self.event_sender.send(msg_event).expect("Cannot send");
        } else {
            error!("Bad key {:?}!", event.code);
        }
    }

    fn mouse_move(&mut self, event: &MouseEvent) {
        debug!("mouse move: {:?}", event);
        let eventmove = tunnel::EventMove {
            x: (event.pos.x * SCALE_X) as u32,
            y: (event.pos.y * SCALE_Y) as u32,
        };
        let msg_event = tunnel::MessageClient {
            msg: Some(tunnel::message_client::Msg::Move(eventmove)),
        };
        self.event_sender.send(msg_event).expect("Cannot send");
    }

    fn mouse_down(&mut self, event: &MouseEvent) {
        debug!("mouse down: {:?}", event);
        let button = match event.button {
            MouseButton::None => {
                warn!("Strange button");
                return;
            }
            MouseButton::Left => 1,
            MouseButton::Right => 2,
            MouseButton::Middle => 3,
            MouseButton::X1 => 4,
            MouseButton::X2 => 5,
        };
        let eventbutton = tunnel::EventButton {
            x: (event.pos.x * SCALE_X) as u32,
            y: (event.pos.y * SCALE_Y) as u32,
            button: button,
            updown: true,
        };
        let msg_event = tunnel::MessageClient {
            msg: Some(tunnel::message_client::Msg::Button(eventbutton)),
        };
        self.event_sender.send(msg_event).expect("Cannot send");
    }

    fn mouse_up(&mut self, event: &MouseEvent) {
        debug!("mouse up: {:?}", event);
        let button = match event.button {
            MouseButton::None => {
                warn!("Strange button");
                return;
            }
            MouseButton::Left => 1,
            MouseButton::Right => 2,
            MouseButton::Middle => 3,
            MouseButton::X1 => 4,
            MouseButton::X2 => 5,
        };
        let eventbutton = tunnel::EventButton {
            x: (event.pos.x * SCALE_X) as u32,
            y: (event.pos.y * SCALE_Y) as u32,
            button: button,
            updown: false,
        };
        let msg_event = tunnel::MessageClient {
            msg: Some(tunnel::message_client::Msg::Button(eventbutton)),
        };
        self.event_sender.send(msg_event).expect("Cannot send");
    }

    fn lost_focus(&mut self) {
        for (index, state) in self.keys_state.iter_mut().enumerate() {
            if *state {
                *state = false;
                let eventkey = tunnel::EventKey {
                    keycode: index as u32,
                    updown: false,
                };
                let msg_event = tunnel::MessageClient {
                    msg: Some(tunnel::message_client::Msg::Key(eventkey)),
                };
                self.event_sender.send(msg_event).expect("Cannot send");
            }
        }
    }

    fn size(&mut self, size: Size) {
        self.size = size;
    }

    fn request_close(&mut self) {
        self.handle.close();
    }

    fn destroy(&mut self) {
        Application::global().quit()
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

pub fn set_img(client_info: &mut ClientInfo, img: &[u8], size: (u32, u32)) -> Result<()> {
    client_info
        .frame_sender
        .send((img.to_owned(), size.0, size.1))
        .map_err(|err| ClientGraphicError::FrameSendError(err))
}

pub fn set_cursor(
    client_info: &mut ClientInfo,
    cursor_data: &[u8],
    size: (u32, u32),
    hot: (u16, u16),
) -> Result<()> {
    client_info
        .cursor_sender
        .send((
            cursor_data.to_owned(),
            size.0,
            size.1,
            hot.0 as i32,
            hot.1 as i32,
        ))
        .map_err(|err| ClientGraphicError::CursorSendError(err))
}

pub fn update(_client_info: &mut ClientInfo, _areas: &HashMap<usize, Area>) {
    // skip
}

pub fn init_druid(server_size: Option<(u16, u16)>) -> Result<ClientInfo> {
    let (client_info, frame_receiver, event_sender, cursor_receiver) =
        ClientInfo::build(server_size);
    let (width, height) = (client_info.width, client_info.height);
    thread::spawn(move || {
        let video_app = VideoApp::new(
            frame_receiver,
            event_sender,
            cursor_receiver,
            width as u32,
            height as u32,
        );
        let app = Application::new().unwrap();
        let mut builder = WindowBuilder::new(app.clone());
        builder.set_handler(Box::new(video_app));
        builder.set_title("Performance tester");
        let window = builder.build().unwrap();
        window.show();
        app.run(None);
    });
    Ok(client_info)
}
