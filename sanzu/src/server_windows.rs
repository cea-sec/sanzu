use crate::{
    config::ConfigServer,
    server_utils::Server,
    utils::{ArgumentsSrv, ServerEvent},
    utils_win,
    video_encoder::{Encoder, EncoderTimings},
};
use anyhow::{Context, Result};

use clipboard::{ClipboardContext, ClipboardProvider};

use sanzu_common::tunnel;

use std::{
    ffi::CString,
    ptr::null_mut,
    sync::{
        mpsc::{channel, Receiver, Sender},
        Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use spin_sleep::sleep;

use winapi::{
    ctypes::c_void,
    shared::{
        minwindef::{LPARAM, LRESULT, UINT, WPARAM},
        windef::HWND,
    },
    um::{
        libloaderapi::GetModuleHandleA,
        wingdi::{
            BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, GetDIBits, GetDeviceCaps,
            GetObjectA, SelectObject, BITMAP, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HORZRES,
            SRCCOPY, VERTRES,
        },
        winuser::{
            CreateWindowExA, DefWindowProcA, DispatchMessageA, GetDC, PeekMessageA,
            RegisterClassExA, SendInput, SetClipboardViewer, TranslateMessage, INPUT,
            INPUT_KEYBOARD, INPUT_MOUSE, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
            KEYEVENTF_SCANCODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
            MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
            MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MSG, PM_REMOVE, WM_DRAWCLIPBOARD, WM_KILLFOCUS,
            WM_QUIT, WM_SETFOCUS, WNDCLASSEXA, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_DLGFRAME,
            WS_POPUP, WS_VISIBLE,
        },
    },
};

lazy_static! {
    // TODO XXX: how to share handle?
    // This variables are initialized once and won't be changed.
    static ref WINHANDLE: Mutex<u64> = Mutex::new(0);
    static ref EVENT_SENDER: Mutex<Option<Sender<tunnel::MessageSrv>>> = Mutex::new(None);

}

/// Holds information on the server
pub struct ServerInfo {
    pub img: Option<Vec<u8>>,
    /// Frame rate limit (see config)
    pub max_stall_img: u32,
    /// Current number of identical server frames
    pub frozen_frames_count: u32,
    /// Number of encoded frames
    pub img_count: i64,
    /// Screen width
    pub width: u16,
    /// Screen height
    pub height: u16,
    pub event_receiver: Receiver<tunnel::MessageSrv>,
}

pub extern "system" fn custom_wnd_proc(
    hwnd: HWND,
    msg: UINT,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_SETFOCUS => {
            trace!("got focus");
        }
        WM_KILLFOCUS => {
            trace!("lost focus");
        }

        WM_DRAWCLIPBOARD => {
            info!("clipboard!");
            let mut ctx: ClipboardContext = ClipboardProvider::new().unwrap();
            if let Ok(data) = ctx.get_contents() {
                info!("Send clipboard {}", data);

                let eventclipboard = tunnel::EventClipboard { data };
                let msg_event = tunnel::MessageSrv {
                    msg: Some(tunnel::message_srv::Msg::Clipboard(eventclipboard)),
                };
                EVENT_SENDER
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .send(msg_event)
                    .expect("Cannot send event clipboard");
            }
        }

        _ => {
            trace!("msg: {:?}", msg);
        }
    }
    unsafe { DefWindowProcA(hwnd, msg, wparam, lparam) }
}

fn set_mouse_position(server_info: &mut ServerInfo, event_x: u32, event_y: u32) {
    let mut input = INPUT {
        type_: INPUT_MOUSE,
        ..Default::default()
    };
    {
        let mut mouse = unsafe { input.u.mi_mut() };
        let fx = event_x as f32 * (65535.0 / server_info.width as f32);
        let fy = event_y as f32 * (65535.0 / server_info.height as f32);
        mouse.dx = fx as i32;
        mouse.dy = fy as i32;
        mouse.dwFlags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE;
        mouse.time = 0;
        mouse.dwExtraInfo = 0;
    }
    let mut inputs = vec![input];
    let inputs_ptr = inputs.as_mut_ptr();
    unsafe {
        SendInput(1, inputs_ptr, std::mem::size_of::<INPUT>() as i32);
    };
}

pub fn init_win(
    _arguments: &ArgumentsSrv,
    config: &ConfigServer,
    _server_size: Option<(u16, u16)>,
) -> Result<Box<dyn Server>> {
    let (screen_width, screen_height) = unsafe {
        let hdc_source = GetDC(null_mut());
        let cap_x = GetDeviceCaps(hdc_source, HORZRES);
        let cap_y = GetDeviceCaps(hdc_source, VERTRES);
        (cap_x as u16, cap_y as u16)
    };
    let (event_sender, event_receiver) = channel();

    thread::spawn(move || {
        let instance_handle = unsafe { GetModuleHandleA(null_mut()) };
        info!("Create window {} {}", screen_width, screen_height);
        let class_name = CString::new("D3D").expect("Couldnt create CString");
        let class_name_ptr = class_name.as_ptr();

        let wc = WNDCLASSEXA {
            cbSize: std::mem::size_of::<WNDCLASSEXA>() as u32,
            hbrBackground: null_mut(),
            lpfnWndProc: Some(custom_wnd_proc),
            lpszClassName: class_name_ptr,
            hInstance: instance_handle,
            ..Default::default()
        };

        let ret = unsafe { RegisterClassExA(&wc) };
        if ret == 0 {
            panic!("Cannot register class");
        }

        let window_name = CString::new("D3D").expect("Couldn't create CString for window name");
        let window_name_ptr = window_name.as_ptr();
        let window: HWND = unsafe {
            CreateWindowExA(
                0,
                wc.lpszClassName,
                window_name_ptr,
                WS_VISIBLE | WS_POPUP | WS_DLGFRAME | WS_CLIPSIBLINGS | WS_CLIPCHILDREN,
                0,
                0,
                100,
                100,
                null_mut(),
                null_mut(),
                instance_handle,
                null_mut(),
            )
        };
        *WINHANDLE.lock().unwrap() = window as u64;
        EVENT_SENDER.lock().unwrap().replace(event_sender);

        unsafe { SetClipboardViewer(window) };
        // Use drop to keep lifetime of original object through unsafe call
        drop(window_name);
        drop(class_name);
        if window.is_null() {
            panic!("Cannot create window");
        }
        debug!("Create window ok {:?}", window);

        let mut msg = MSG::default();
        while msg.message != WM_QUIT {
            while unsafe { PeekMessageA(&mut msg as *mut _, null_mut(), 0, 0, PM_REMOVE) } != 0 {
                unsafe { TranslateMessage(&msg) };
                unsafe { DispatchMessageA(&msg) };
            }
            sleep(Duration::from_millis(5));
        }
    });

    let server = ServerInfo {
        img: None,
        max_stall_img: config.video.max_stall_img,
        frozen_frames_count: 0,
        img_count: 0,
        width: screen_width,
        height: screen_height,
        event_receiver,
    };
    Ok(Box::new(server))
}

impl Server for ServerInfo {
    fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    fn grab_frame(&mut self) -> Result<()> {
        unsafe {
            let time_start = Instant::now();

            let hdc_source = GetDC(null_mut());
            let hdc_memory = CreateCompatibleDC(hdc_source);
            let cap_x = GetDeviceCaps(hdc_source, HORZRES);
            let cap_y = GetDeviceCaps(hdc_source, VERTRES);

            let h_bitmap = CreateCompatibleBitmap(hdc_source, cap_x, cap_y);
            let _h_bitmap_old = SelectObject(hdc_memory, h_bitmap as *mut c_void);

            let time_getdc = Instant::now();

            BitBlt(hdc_memory, 0, 0, cap_x, cap_y, hdc_source, 0, 0, SRCCOPY);

            let time_bitblt = Instant::now();

            let mut bitmap_0 = BITMAP::default();
            let ptr = &mut bitmap_0;
            GetObjectA(
                h_bitmap as *mut c_void,
                std::mem::size_of::<BITMAP>() as i32,
                std::mem::transmute(ptr),
            );
            debug!("obk {:?} {:?}", bitmap_0.bmWidth, bitmap_0.bmHeight);

            let mut bi = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: bitmap_0.bmWidth,
                biHeight: bitmap_0.bmHeight,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrImportant: 0,
                biClrUsed: 256,
            };

            let dw_bm_bits_size =
                ((bitmap_0.bmWidth * bi.biBitCount as i32 + 31) & !31) / 8 * bitmap_0.bmHeight;
            let mut data = vec![0u8; dw_bm_bits_size as usize];
            let data_ptr = data.as_mut_ptr();

            let bi_ptr = &mut bi;
            GetDIBits(
                hdc_source,
                h_bitmap,
                0,
                bitmap_0.bmHeight as u32,
                data_ptr as *mut c_void,
                std::mem::transmute(bi_ptr),
                DIB_RGB_COLORS,
            );
            let time_stop = Instant::now();
            debug!(
                "grab time: {:?} {:?} {:?}",
                time_stop - time_bitblt,
                time_bitblt - time_getdc,
                time_getdc - time_start
            );

            // invert image
            let mut data_inverted = vec![0u8; data.len()];
            for i in 0..bitmap_0.bmHeight as usize {
                let index = bitmap_0.bmWidth as usize * 4;
                let j = bitmap_0.bmHeight as usize - i - 1;
                data_inverted[i * index..(i + 1) * index]
                    .copy_from_slice(&data[j * index..(j + 1) * index]);
            }
            self.img = Some(data_inverted);
            drop(data);
            DeleteDC(hdc_source);
            DeleteDC(hdc_memory);
        };

        Ok(())
    }

    fn handle_client_event(&mut self, msgs: tunnel::MessagesClient) -> Result<Vec<ServerEvent>> {
        let mut server_events = vec![];
        for msg in msgs.msgs.iter() {
            //info!("MSG {:?}", msg);
            match &msg.msg {
                Some(tunnel::message_client::Msg::Move(event)) => {
                    info!("Mouse move {} {}", event.x, event.y);
                    set_mouse_position(self, event.x, event.y);
                }
                Some(tunnel::message_client::Msg::Button(event)) => {
                    info!(
                        "Mouse button {} {} {} {}",
                        event.x, event.y, event.button, event.updown
                    );
                    // First mouve
                    set_mouse_position(self, event.x, event.y);
                    // Then click
                    let mut input = INPUT {
                        type_: INPUT_MOUSE,
                        ..Default::default()
                    };
                    input.type_ = INPUT_MOUSE;
                    {
                        let mut mouse = unsafe { input.u.mi_mut() };
                        mouse.mouseData = event.button;
                        mouse.dwFlags = 0;
                        match (event.button, event.updown) {
                            (1, true) => {
                                // left down
                                mouse.dwFlags |= MOUSEEVENTF_LEFTDOWN
                            }
                            (1, false) => {
                                // left up
                                mouse.dwFlags |= MOUSEEVENTF_LEFTUP
                            }
                            (2, true) => {
                                // middle down
                                mouse.dwFlags |= MOUSEEVENTF_MIDDLEDOWN
                            }
                            (2, false) => {
                                // middle up
                                mouse.dwFlags |= MOUSEEVENTF_MIDDLEUP
                            }
                            (3, true) => {
                                // right down
                                mouse.dwFlags |= MOUSEEVENTF_RIGHTDOWN
                            }
                            (3, false) => {
                                // right up
                                mouse.dwFlags |= MOUSEEVENTF_RIGHTUP
                            }
                            (4, true) => {
                                // wheel up
                                mouse.mouseData = 40;
                                mouse.dwFlags |= MOUSEEVENTF_WHEEL
                            }
                            (4, false) => {
                                // wheel up end
                                mouse.mouseData = 40;
                                mouse.dwFlags |= MOUSEEVENTF_WHEEL
                            }
                            (5, true) => {
                                // wheel down
                                mouse.mouseData = -40i32 as u32;
                                mouse.dwFlags |= MOUSEEVENTF_WHEEL
                            }
                            (5, false) => {
                                // wheel down end
                                mouse.mouseData = -40i32 as u32;
                                mouse.dwFlags |= MOUSEEVENTF_WHEEL
                            }
                            (a, b) => {
                                warn!("unhandlerd {:?} {:?}", a, b);
                            }
                        }
                        mouse.time = 0;
                        mouse.dwExtraInfo = 0;
                    }
                    let mut inputs = vec![input];
                    let inputs_ptr = inputs.as_mut_ptr();
                    unsafe {
                        SendInput(1, inputs_ptr, std::mem::size_of::<INPUT>() as i32);
                    };
                }
                Some(tunnel::message_client::Msg::Key(event)) => {
                    if let Some((keycode, extened)) =
                        utils_win::hardware_keycode_to_hid_code(event.keycode)
                    {
                        let mut input = INPUT {
                            type_: INPUT_KEYBOARD,
                            ..Default::default()
                        };

                        {
                            let mut keyb = unsafe { input.u.ki_mut() };
                            keyb.wScan = keycode;
                            if !event.updown {
                                keyb.dwFlags |= KEYEVENTF_KEYUP;
                            }
                            keyb.dwFlags |= KEYEVENTF_SCANCODE;
                            if extened {
                                keyb.dwFlags |= KEYEVENTF_EXTENDEDKEY;
                            }
                            keyb.time = 0;
                            keyb.dwExtraInfo = 0;
                        }
                        let mut inputs = vec![input];
                        let inputs_ptr = inputs.as_mut_ptr();
                        unsafe {
                            SendInput(1, inputs_ptr, std::mem::size_of::<INPUT>() as i32);
                        };
                        drop(inputs);
                    }
                }
                Some(tunnel::message_client::Msg::Display(event)) => {
                    server_events.push(ServerEvent::ResolutionChange(event.width, event.height));
                }
                _ => {}
            }
        }
        Ok(server_events)
    }

    fn poll_events(&mut self) -> Result<Vec<tunnel::MessageSrv>> {
        let mut events = vec![];
        while let Ok(event) = self.event_receiver.try_recv() {
            events.push(event);
        }

        Ok(events)
    }

    fn generate_encoded_img(
        &mut self,
        video_encoder: &mut Box<dyn Encoder>,
    ) -> Result<(Vec<tunnel::MessageSrv>, Option<EncoderTimings>)> {
        let mut events = vec![];
        let mut timings = None;
        if self.frozen_frames_count < self.max_stall_img {
            if let Some(ref data) = &self.img {
                let (width, height) = (self.width as u32, self.height as u32);
                let result = video_encoder
                    .encode_image(data, width, height, width * 4, self.img_count)
                    .context("Error in encode image")?;

                let encoded = result.0;
                timings = Some(result.1);

                /* Prepare encoded image */
                let img = if video_encoder.is_raw() {
                    tunnel::message_srv::Msg::ImgRaw(tunnel::ImageRaw {
                        data: encoded,
                        width,
                        height,
                        bytes_per_line: width * 4,
                    })
                } else {
                    tunnel::message_srv::Msg::ImgEncoded(tunnel::ImageEncoded {
                        data: encoded,
                        width,
                        height,
                    })
                };
                let msg_img = tunnel::MessageSrv { msg: Some(img) };
                events.push(msg_img);
            }
        }

        Ok((events, timings))
    }

    fn change_resolution(
        &mut self,
        _config: &ConfigServer,
        _width: u32,
        _height: u32,
    ) -> Result<()> {
        error!("Unsupported os");
        Ok(())
    }

    fn activate_window(&self, _win_id: u32) -> Result<()> {
        Ok(())
    }
}
