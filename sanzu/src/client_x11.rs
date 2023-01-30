use crate::{
    client_utils::{Area, Client},
    utils::{ArgumentsClient, ClipboardConfig, ClipboardSelection},
    utils_x11,
};
use anyhow::{Context, Result};
use lock_keys::LockKeyWrapper;
use sanzu_common::tunnel;

use std::{
    collections::HashMap,
    sync::{
        mpsc::{channel, Receiver},
        Arc, Mutex,
    },
    thread,
};

use utils_x11::{get_clipboard_events, listen_clipboard};
use x11_clipboard::Clipboard;

use x11rb::{
    connection::{Connection, RequestConnection},
    protocol::{
        bigreq::{self, ConnectionExt as _},
        randr::{self, ConnectionExt as _},
        shape::{self, ConnectionExt as _},
        shm::{self, ConnectionExt as _},
        xfixes::ConnectionExt as _,
        xproto::ConnectionExt as _,
        xproto::*,
        Event,
    },
    rust_connection::{DefaultStream, RustConnection},
    wrapper::ConnectionExt as _,
    COPY_DEPTH_FROM_PARENT,
};

/// xkbprint -color -kc :0 - | ps2pdf - > xkbprint.pdf
const KEY_CTRL: usize = 37;
const KEY_SHIFT: usize = 50;
const KEY_ALT: usize = 64;
const KEY_S: usize = 39;
const KEY_C: usize = 54;

struct FreePixmap<'c, C: Connection>(&'c C, Pixmap);
impl<C: Connection> Drop for FreePixmap<'_, C> {
    fn drop(&mut self) {
        self.0.free_pixmap(self.1).unwrap();
    }
}
struct FreeGC<'c, C: Connection>(&'c C, Gcontext);
impl<C: Connection> Drop for FreeGC<'_, C> {
    fn drop(&mut self) {
        self.0.free_gc(self.1).unwrap();
    }
}

/// Get the supported SHM version from the X11 server
fn check_shm_version<C: Connection>(conn: &C) -> Result<(u16, u16)> {
    conn.extension_information(shm::X11_EXTENSION_NAME)
        .context("Error in get shm extension")?
        .context("Shm must be supported")?;

    let shm_version = conn
        .shm_query_version()
        .context("Error in query shm version")?
        .reply()
        .context("Error in query shm version reply")?;
    Ok((shm_version.major_version, shm_version.minor_version))
}

/// Enable Big Request for 4k resolutions and more
fn enable_big_request<C: Connection>(conn: &C) -> Result<usize> {
    conn.extension_information(bigreq::X11_EXTENSION_NAME)?;

    let response = conn
        .bigreq_enable()
        .context("Error in bigreq test")?
        .reply()
        .context("Error in bigreq reply")?;
    Ok(response.maximum_request_length as usize * 4)
}

/// Holds information on the local client graphic window
pub struct WindowInfo {
    /// x11rb window handle
    pub window: Window,
    /// window size
    pub size: (u16, u16),
    /// window current pixmap
    pub pixmap: Pixmap,
}

/// Holds information on the local client
pub struct ClientInfo {
    /// x11rb connection
    pub conn: RustConnection<DefaultStream>,
    /// Max request size for x11
    pub max_request_size: usize,
    /// Current screen index
    pub screen_num: usize,
    /// Client screen width
    pub width: u16,
    /// Client screen height
    pub height: u16,
    /// Local window information
    /// (local size if seamless, else remote screen size)
    pub window_info: WindowInfo,
    /// Black graphic context
    pub black_gc: Gcontext,
    /// Information on current up/down keys
    pub keys_state: Vec<bool>,
    /// graphic windows needs an update in the net client frame
    pub need_update: bool,
    /// Are we in seamless
    pub seamless: bool,
    /// Clipboard instance,
    pub clipboard: Clipboard,
    /// Clipboard behavior
    pub clipboard_config: ClipboardConfig,
    /// Clipboard event receiver
    pub clipboard_event_receiver: Receiver<String>,
    /// Last seen clipboard value
    pub clipboard_last_value: Option<String>,
    /// store clipboard events to skip
    pub skip_clipboard_primary: Arc<Mutex<u32>>,
    pub skip_clipboard_clipboard: Arc<Mutex<u32>>,
    pub display_stats: bool,
    /// Bool to trig clipboard send
    pub clipbard_trig: bool,
    /// Sync caps/num/scroll lock
    pub sync_key_locks: bool,
    /// is key lock sync needed
    pub sync_key_locks_needed: bool,
    /// Stores windows
    pub areas: Vec<(usize, Area)>,
}

fn create_gc<C: Connection>(
    conn: &C,
    win_id: Window,
    foreground: u32,
    background: u32,
) -> Result<Gcontext> {
    let gc = conn.generate_id()?;
    let gc_aux = CreateGCAux::new()
        .graphics_exposures(1)
        .foreground(foreground)
        .background(background);
    conn.create_gc(gc, win_id, &gc_aux)
        .context("Error in create gc")?;
    Ok(gc)
}

fn setup_window<C: Connection>(
    conn: &C,
    arguments: &ArgumentsClient,
    screen: &Screen,
    window_position: (i16, i16),
    window_size: (u16, u16),
) -> Result<Window> {
    let win_id = conn.generate_id().context("Error in generate_id")?;
    let win_aux = CreateWindowAux::new()
        .event_mask(
            EventMask::POINTER_MOTION
                | EventMask::STRUCTURE_NOTIFY
                | EventMask::KEY_PRESS
                | EventMask::KEY_RELEASE
                | EventMask::BUTTON_PRESS
                | EventMask::BUTTON_RELEASE
                | EventMask::PROPERTY_CHANGE
                | EventMask::FOCUS_CHANGE,
        )
        .background_pixel(screen.white_pixel);

    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win_id,
        screen.root,
        window_position.0,
        window_position.1,
        window_size.0,
        window_size.1,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &win_aux,
    )
    .context("Error in create_window")?;

    let title = "Surf client";
    if let Err(err) = conn
        .change_property8(
            PropMode::REPLACE,
            win_id,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            title.as_bytes(),
        )
        .context("Error on change window name")
    {
        err.chain().for_each(|cause| error!(" - due to {}", cause));
    }

    if !arguments.window_mode {
        let wm_state = conn
            .intern_atom(true, b"_NET_WM_STATE")
            .context("Error in intern_atom")?
            .reply()
            .context("Error in intern_atom reply")?
            .atom;

        let wm_full = conn
            .intern_atom(true, b"_NET_WM_STATE_FULLSCREEN")
            .context("Error in intern_atom")?
            .reply()
            .context("Error in intern_atom reply")?
            .atom;

        if let Err(err) = conn
            .change_property32(
                PropMode::REPLACE,
                win_id,
                wm_state,
                AtomEnum::ATOM,
                &[wm_full],
            )
            .context("Error in change_property32")
            .map(|reply| reply.check())
            .context("Error in change_property32 check")
        {
            error!("Change full screen error");
            err.chain().for_each(|cause| error!(" - due to {}", cause));
        }
    }

    conn.map_window(win_id)
        .context("Error in map_window")?
        .check()
        .context("Error in map_window check")?;

    conn.flush().context("Error in x11rb flush")?;

    Ok(win_id)
}

fn new_area<C: Connection>(
    conn: &C,
    arguments: &ArgumentsClient,
    screen: &Screen,
    size: (u16, u16),
) -> Result<WindowInfo> {
    let win_id =
        setup_window(conn, arguments, screen, (0, 0), size).context("Error in setup_window")?;

    let pixmap = conn.generate_id().context("Error in x11rb generate_id")?;

    conn.create_pixmap(screen.root_depth, pixmap, win_id, size.0, size.1)
        .context("Error in x11rb create_pixmap")?;

    let window_info = WindowInfo {
        window: win_id,
        size,
        pixmap,
    };

    let gc_id = conn.generate_id().context("Error in x11rb generate_id")?;
    conn.create_gc(gc_id, window_info.window, &CreateGCAux::default())
        .context("Error in create_gc")?;

    conn.flush().context("Error in x11rb flush")?;
    Ok(window_info)
}

/// Initialize the x11 client window
///
/// Initialize the x11 xfixes extension to support clipboard manipulations
/// Initialize the x11 shape extension to support custom shaped windows (used in
/// the seamless version)
pub fn init_x11rb(
    arguments: &ArgumentsClient,
    seamless: bool,
    server_size: Option<(u16, u16)>,
) -> Result<Box<dyn Client>> {
    debug!("Start client");
    let (conn, screen_num) =
        RustConnection::connect(None).context("Failed to connect to the X11 server")?;

    let setup = conn.setup();
    let screen = &setup.roots[screen_num];

    let screen_width = screen.width_in_pixels;
    let screen_height = screen.height_in_pixels;

    let (width, height) = if let Some((width, height)) = server_size {
        (width, height)
    } else {
        (screen_width, screen_height)
    };

    // Force the resolution to be less thant the server side
    let width = width.min(screen_width);
    let height = height.min(screen_height);

    // Check for SHM 1.2 support (needed for fd passing)
    let (major, minor) = check_shm_version(&conn).context("Error in check_shm_version")?;
    if major < 1 || (major == 1 && minor < 2) {
        let err = format!(
            "X11 server supports version {major}.{minor} of the SHM extension, but version 1.2 \
             is needed",
        );
        return Err(anyhow!(err));
    }

    /* Enable big request for 4k and more */
    let max_request_size =
        enable_big_request(&conn).context("Error in activate big request extension")?;
    debug!("Max request size: {:?}", max_request_size);

    /* Load shape extension */
    conn.extension_information(shape::X11_EXTENSION_NAME)
        .context("failed to get extension information")?
        .context("XShape must be supported")?;

    let setup = conn.setup();
    let screen = &setup.roots[screen_num];
    conn.xfixes_query_version(100, 0)
        .context("Error in xfixes_query_version")?;

    let (selection_sender_primary, clipboard_event_receiver) = channel();
    let selection_sender_clipboard = selection_sender_primary.clone();

    let skip_clipboard_primary = Arc::new(Mutex::new(0));
    let skip_clipboard_clipboard = Arc::new(Mutex::new(0));

    let skip_clipboard_primary_thread = skip_clipboard_primary.clone();
    let skip_clipboard_clipboard_thread = skip_clipboard_clipboard.clone();

    match arguments.clipboard_config {
        ClipboardConfig::Allow | ClipboardConfig::Trig => {
            // Listen "primary" clipboard events
            thread::spawn(move || {
                listen_clipboard(
                    ClipboardSelection::Primary,
                    selection_sender_primary,
                    skip_clipboard_primary_thread,
                );
            });

            // Listen "clipboard" clipboard events
            thread::spawn(move || {
                listen_clipboard(
                    ClipboardSelection::Clipboard,
                    selection_sender_clipboard,
                    skip_clipboard_clipboard_thread,
                );
            });
        }
        _ => {}
    };

    /* randr extension to detect screen resolution changes */
    conn.extension_information(randr::X11_EXTENSION_NAME)
        .context("failed to get extension information")?
        .context("Randr must be supported")?;

    conn.randr_select_input(screen.root, randr::NotifyMask::CRTC_CHANGE)
        .context("Error in randr_select_input")?
        .check()
        .context("Error in randr_select_input check")?;

    let window_info =
        new_area(&conn, arguments, screen, (width, height)).context("Error in new_area")?;

    let black_gc = create_gc(&conn, screen.root, screen.black_pixel, screen.black_pixel)
        .context("Error in create_gc")?;
    let keys_state = vec![false; 0x100];

    let clipboard = Clipboard::new().context("Error in clipboard creation")?;

    let client_info = ClientInfo {
        conn,
        max_request_size,
        screen_num,
        width,
        height,
        window_info,
        black_gc,
        keys_state,
        need_update: true,
        seamless,
        clipboard,
        clipboard_config: arguments.clipboard_config,
        clipboard_event_receiver,
        clipboard_last_value: None,
        skip_clipboard_primary,
        skip_clipboard_clipboard,
        display_stats: false,
        clipbard_trig: false,
        sync_key_locks: arguments.sync_key_locks,
        sync_key_locks_needed: arguments.sync_key_locks,
        areas: vec![],
    };

    Ok(Box::new(client_info))
}

/// Set the client image to `img`, with a size of `width`x`height`x4 (32bpp) in 24bpp (rgb)
fn put_frame(client_info: &mut ClientInfo, img: &[u8], width: u32, height: u32) -> Result<()> {
    if img.len() < client_info.max_request_size {
        client_info
            .conn
            .put_image(
                ImageFormat::Z_PIXMAP,
                client_info.window_info.pixmap,
                client_info.black_gc,
                width as u16,
                height as u16,
                0,
                0,
                0,
                24,
                img,
            )
            .context("Error in put_image")?;
    } else {
        // Our image is bigger than the x11 max request size;
        // Split it into chunks with a size below this limit
        let round_size = 0x10000_usize - 1_usize;
        let max_size = client_info.max_request_size & (!round_size);
        let max_lines = max_size / (4_usize * width as usize);
        let mut lines_rem = height as usize;
        let mut cur_line = 0_usize;
        while lines_rem != 0 {
            let lines_todo = if lines_rem > max_lines {
                max_lines
            } else {
                lines_rem
            };
            let cur_img =
                &img[cur_line * width as usize * 4..(cur_line + lines_todo) * width as usize * 4];
            client_info
                .conn
                .put_image(
                    ImageFormat::Z_PIXMAP,
                    client_info.window_info.pixmap,
                    client_info.black_gc,
                    width as u16,
                    lines_todo as u16,
                    0,
                    cur_line as i16,
                    0,
                    24,
                    cur_img,
                )
                .context("Error in put_image")?;
            cur_line += lines_todo;
            lines_rem -= lines_todo;
        }
    }
    Ok(())
}

fn create_gc_with_foreground<C: Connection>(
    conn: &C,
    win_id: Window,
    foreground: u32,
) -> Result<Gcontext> {
    let gc = conn.generate_id()?;
    let gc_aux = CreateGCAux::new()
        .graphics_exposures(0)
        .foreground(foreground);
    conn.create_gc(gc, win_id, &gc_aux)?;
    Ok(gc)
}

fn shape_window(client_info: &mut ClientInfo, areas: &HashMap<usize, Area>) -> Result<()> {
    // Create a pixmap for the shape
    let pixmap = client_info
        .conn
        .generate_id()
        .context("Error in x11rb generate_id")?;
    let window_info = &client_info.window_info;
    client_info
        .conn
        .create_pixmap(
            1,
            pixmap,
            window_info.window,
            window_info.size.0,
            window_info.size.1,
        )
        .context("Error in create_pixmap")?;

    // Fill the pixmap with what will indicate "transparent"
    let gc = create_gc_with_foreground(&client_info.conn, pixmap, 0)
        .context("Error in create_gc_with_foreground")?;

    let rect = Rectangle {
        x: 0,
        y: 0,
        width: window_info.size.0,
        height: window_info.size.1,
    };
    client_info
        .conn
        .poly_fill_rectangle(pixmap, gc, &[rect])
        .context("Error in poly_fill_rectangle")?;

    // Draw as "not transparent"
    let values = ChangeGCAux::new().foreground(1);
    client_info
        .conn
        .change_gc(gc, &values)
        .context("Error in change_gc")?;

    let mut rects = vec![];
    for (_, area) in areas.iter() {
        if area.mapped {
            let rect = Rectangle {
                x: area.position.0,
                y: area.position.1,
                width: area.size.0,
                height: area.size.1,
            };
            rects.push(rect);
        }
    }

    client_info
        .conn
        .poly_fill_rectangle(pixmap, gc, &rects)
        .context("Error in poly_fill_rectangle")?;

    // Set the shape of the window
    client_info
        .conn
        .shape_mask(
            shape::SO::SET,
            shape::SK::BOUNDING,
            client_info.window_info.window,
            0,
            0,
            pixmap,
        )
        .context("Error in shape_mask")?;

    client_info.conn.free_gc(gc).context("Error in free_gc")?;
    client_info
        .conn
        .free_pixmap(pixmap)
        .context("Error in free_pixmap")?;

    Ok(())
}

fn key_state_to_bool(state: lock_keys::LockKeyState) -> bool {
    match state {
        lock_keys::LockKeyState::Enabled => true,
        lock_keys::LockKeyState::Disabled => false,
    }
}

impl Client for ClientInfo {
    fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    fn set_cursor(&mut self, cursor_data: &[u8], size: (u32, u32), hot: (u16, u16)) -> Result<()> {
        let cid = self
            .conn
            .generate_id()
            .context("Error in x11rb generate_id")?;

        let pixmap_data = self
            .conn
            .generate_id()
            .context("Error in x11rb generate_id")?;
        let pixmap_mask = self
            .conn
            .generate_id()
            .context("Error in x11rb generate_id")?;

        // TODO XXX: the client support only 1 bit cursor for now
        let pixmap_data = FreePixmap(&self.conn, pixmap_data);
        let pixmap_mask = FreePixmap(&self.conn, pixmap_mask);

        self.conn
            .create_pixmap(
                1,
                pixmap_data.1,
                self.window_info.window,
                size.0 as u16,
                size.1 as u16,
            )
            .context("Error in create_pixmap")?
            .check()
            .context("Error in create_pixmap check")?;

        self.conn
            .create_pixmap(
                1,
                pixmap_mask.1,
                self.window_info.window,
                size.0 as u16,
                size.1 as u16,
            )
            .context("Error in create_pixmap")?
            .check()
            .context("Error in create_pixmap check")?;

        let black_gc = create_gc(&self.conn, pixmap_data.1, 0, 0).context("Error in create_gc")?;
        let white_gc =
            create_gc(&self.conn, pixmap_data.1, 0x1, 0x1).context("Error in create_gc")?;
        let black_gc = FreeGC(&self.conn, black_gc);
        let white_gc = FreeGC(&self.conn, white_gc);

        let mut pixels_white = vec![];
        let mut pixels_black = vec![];
        let mut pixels_mask = vec![];
        for x in 0..size.0 {
            for y in 0..size.1 {
                let pixel_r = cursor_data[((x + y * size.0) * 4) as usize];
                let pixel_g = cursor_data[((x + y * size.0) * 4 + 1) as usize];
                let pixel_b = cursor_data[((x + y * size.0) * 4 + 2) as usize];
                let pixel_a = cursor_data[((x + y * size.0) * 4 + 3) as usize];
                let color = (pixel_r as u32 + pixel_g as u32 + pixel_b as u32) / 3;

                if color > 0x70 {
                    pixels_white.push(Point {
                        x: x as i16,
                        y: y as i16,
                    });
                } else {
                    pixels_black.push(Point {
                        x: x as i16,
                        y: y as i16,
                    });
                }
                if pixel_a > 0x30 {
                    pixels_mask.push(Point {
                        x: x as i16,
                        y: y as i16,
                    });
                }
            }
        }
        self.conn
            .poly_point(CoordMode::ORIGIN, pixmap_data.1, white_gc.1, &pixels_white)
            .context("Error in poly_point")?
            .check()
            .context("Error in poly_point check")?;
        self.conn
            .poly_point(CoordMode::ORIGIN, pixmap_data.1, black_gc.1, &pixels_black)
            .context("Error in poly_point")?
            .check()
            .context("Error in poly_point check")?;
        self.conn
            .poly_point(CoordMode::ORIGIN, pixmap_mask.1, white_gc.1, &pixels_mask)
            .context("Error in poly_point")?
            .check()
            .context("Error in poly_point check")?;

        /*
        TODO XXX: the cursor createion seems to support pixmap of
        data & mask of the same depth. As the mask depth is 1, the
        data pixmap seems locked to 1 bit depth. This implies ugly 1
        bit depth cursors
         */
        self.conn
            .create_cursor(
                cid,
                pixmap_data.1,
                pixmap_mask.1,
                0xffff,
                0xffff,
                0xffff,
                0x0000,
                0x0000,
                0x0000,
                hot.0,
                hot.1,
            )
            .context("Error in create_cursor")?
            .check()
            .context("Error in create_cursor check")?;

        let values = ChangeWindowAttributesAux::default().cursor(Some(cid));
        self.conn
            .change_window_attributes(self.window_info.window, &values)
            .context("Error in change_window_attributes")?
            .check()
            .context("Error in change_window_attributes check")?;

        self.conn.flush().context("Error in x11rb flush")?;
        self.conn
            .free_cursor(cid)
            .context("Error in free_cursor")?
            .check()
            .context("Error in free_cursor check")?;
        Ok(())
    }

    fn set_img(&mut self, img: &[u8], size: (u32, u32)) -> Result<()> {
        self.need_update = true;
        put_frame(self, img, size.0, size.1)
    }

    fn update(&mut self, areas: &HashMap<usize, Area>) -> Result<()> {
        if self.need_update {
            if self.seamless {
                let mut distant_areas: Vec<(usize, Area)> =
                    areas.iter().map(|(a, b)| (*a, b.clone())).collect();
                distant_areas.sort();
                if distant_areas != self.areas {
                    for area in distant_areas.iter() {
                        trace!("    {:?}", area);
                    }
                    shape_window(self, areas).context("Error in shape_window")?;
                    self.areas = distant_areas;
                }
            }
            self.conn
                .copy_area(
                    self.window_info.pixmap,
                    self.window_info.window,
                    self.black_gc,
                    0,
                    0,
                    0,
                    0,
                    self.window_info.size.0,
                    self.window_info.size.1,
                )
                .context("Error in copy_area")?;
            self.need_update = false;
            self.conn.flush().context("Error in x11rb flush")?;
        }
        Ok(())
    }

    fn set_clipboard(&mut self, data: &str) -> Result<()> {
        /* Set *both* clipboards (primary and clipboard) */
        *self.skip_clipboard_clipboard.lock().unwrap() += 1;
        utils_x11::set_clipboard(&self.clipboard, 0, data).context("Error in set_clipboard")?;

        *self.skip_clipboard_primary.lock().unwrap() += 1;
        utils_x11::set_clipboard(&self.clipboard, 1, data).context("Error in set_clipboard")?;

        self.conn.flush().context("Error in x11rb flush")?;

        Ok(())
    }

    fn poll_events(&mut self) -> Result<tunnel::MessagesClient> {
        let mut events = vec![];
        let mut last_move = None;
        let mut last_resize = None;
        self.need_update = false;

        if self.sync_key_locks && self.sync_key_locks_needed {
            let lockkey = lock_keys::LockKey::new();
            let caps_lock = lockkey
                .state(lock_keys::LockKeys::CapitalLock)
                .expect("Cannot get key state");
            let num_lock = lockkey
                .state(lock_keys::LockKeys::NumberLock)
                .expect("Cannot get key state");
            let scroll_lock = lockkey
                .state(lock_keys::LockKeys::ScrollingLock)
                .expect("Cannot get key state");

            let eventkeysync = tunnel::EventKeyLocks {
                caps_lock: key_state_to_bool(caps_lock),
                num_lock: key_state_to_bool(num_lock),
                scroll_lock: key_state_to_bool(scroll_lock),
            };

            let msg_event = tunnel::MessageClient {
                msg: Some(tunnel::message_client::Msg::Keylocks(eventkeysync)),
            };
            events.push(msg_event);

            self.sync_key_locks_needed = false;
        }

        while let Some(event) = self
            .conn
            .poll_for_event()
            .context("Error in poll_for_event")?
        {
            match event {
                Event::MotionNotify(event) => {
                    trace!("Mouse move");
                    let eventmove = tunnel::EventMove {
                        x: event.event_x as u32,
                        y: event.event_y as u32,
                    };

                    /* If multiple mose moves, keep only last one */
                    last_move = Some(tunnel::MessageClient {
                        msg: Some(tunnel::message_client::Msg::Move(eventmove)),
                    });
                }

                Event::ButtonPress(event) => {
                    trace!("Mouse button down {}", event.detail);
                    let eventbutton = tunnel::EventButton {
                        x: event.event_x as u32,
                        y: event.event_y as u32,
                        button: event.detail as u32,
                        updown: true,
                    };
                    let msg_event = tunnel::MessageClient {
                        msg: Some(tunnel::message_client::Msg::Button(eventbutton)),
                    };
                    events.push(msg_event);
                }
                Event::ButtonRelease(event) => {
                    trace!("Mouse button up {}", event.detail);
                    let eventbutton = tunnel::EventButton {
                        x: event.event_x as u32,
                        y: event.event_y as u32,
                        button: event.detail as u32,
                        updown: false,
                    };
                    let msg_event = tunnel::MessageClient {
                        msg: Some(tunnel::message_client::Msg::Button(eventbutton)),
                    };
                    events.push(msg_event);
                }

                Event::KeyPress(event) => {
                    trace!("Key down {:?}", event.detail as u32 & 0xFF);
                    self.keys_state[(event.detail as u32 & 0xFF) as usize] = true;

                    // If Ctrl alt shift s => Generate toggle server logs
                    if event.detail == KEY_S as u8 {
                        // Ctrl Shift Alt
                        if self.keys_state[KEY_CTRL]
                            && self.keys_state[KEY_SHIFT]
                            && self.keys_state[KEY_ALT]
                        {
                            self.display_stats = !self.display_stats;
                            info!("Toggle server logs");
                        }
                    }

                    // If Ctrl alt shift c => Trig clipboard event
                    if event.detail == KEY_C as u8 {
                        // Ctrl Shift Alt
                        if self.keys_state[KEY_CTRL]
                            && self.keys_state[KEY_SHIFT]
                            && self.keys_state[KEY_ALT]
                        {
                            self.clipbard_trig = true;
                        }
                    }

                    let eventkey = tunnel::EventKey {
                        keycode: event.detail as u32,
                        updown: true,
                    };
                    let msg_event = tunnel::MessageClient {
                        msg: Some(tunnel::message_client::Msg::Key(eventkey)),
                    };
                    events.push(msg_event);
                }
                Event::KeyRelease(event) => {
                    trace!("key up");
                    self.keys_state[(event.detail as u32 & 0xFF) as usize] = false;
                    let eventkey = tunnel::EventKey {
                        keycode: event.detail as u32,
                        updown: false,
                    };
                    let msg_event = tunnel::MessageClient {
                        msg: Some(tunnel::message_client::Msg::Key(eventkey)),
                    };
                    events.push(msg_event);
                }
                Event::FocusIn(event) => {
                    trace!("Focus in {:?}", event);
                    let eventwinactivate = tunnel::EventWinActivate { id: 0 };
                    let msg_event = tunnel::MessageClient {
                        msg: Some(tunnel::message_client::Msg::Activate(eventwinactivate)),
                    };
                    events.push(msg_event);

                    self.need_update = true;
                    // If caps/num/scroll locks have changed during out of focus,
                    // force keys state synchro
                    self.sync_key_locks_needed = true;
                }
                Event::FocusOut(_) => {
                    trace!("Focus out");
                    /* On focus out, release each pushed keys */
                    for (index, key_state) in self.keys_state.iter_mut().enumerate() {
                        if *key_state {
                            *key_state = false;
                            let eventkey = tunnel::EventKey {
                                keycode: index as u32,
                                updown: false,
                            };
                            let msg_event = tunnel::MessageClient {
                                msg: Some(tunnel::message_client::Msg::Key(eventkey)),
                            };
                            events.push(msg_event);
                        }
                    }
                }
                Event::NoExposure(_event) => {}
                Event::ConfigureNotify(event) => {
                    warn!("Resize {:?}", event);
                    let (width, height) = (event.width, event.height);
                    if width != self.width || height != self.height {
                        let msg = tunnel::EventDisplay {
                            width: width as u32,
                            height: height as u32,
                        };
                        let msg = tunnel::MessageClient {
                            msg: Some(tunnel::message_client::Msg::Display(msg)),
                        };
                        last_resize = Some(msg);
                        self.width = width;
                        self.height = height;
                    }

                    self.need_update = true;
                }
                Event::MapNotify(event) => {
                    trace!("MapNotify {:?}", event);
                    self.need_update = true;
                }
                Event::RandrNotify(event) => {
                    trace!("RandrNotify {:?}", event);
                }
                Event::Error(_event) => {}
                _ => {
                    warn!("Unknown event {:?}", event);
                }
            }
        }

        /* Get clipboard events */
        if let Some(data) = get_clipboard_events(&self.clipboard_event_receiver) {
            self.clipboard_last_value = Some(data);
        }

        if let Some(last_move) = last_move {
            events.push(last_move);
        }
        if let Some(last_resize) = last_resize {
            events.push(last_resize);
        }

        match self.clipboard_config {
            ClipboardConfig::Deny => {}

            ClipboardConfig::Allow => {
                if let Some(data) = self.clipboard_last_value.take() {
                    let eventclipboard = tunnel::EventClipboard { data };
                    let clipboard_msg = tunnel::MessageClient {
                        msg: Some(tunnel::message_client::Msg::Clipboard(eventclipboard)),
                    };
                    events.push(clipboard_msg);
                }
            }

            ClipboardConfig::Trig => {
                if let (true, Some(ref data)) = (self.clipbard_trig, &self.clipboard_last_value) {
                    // If we triggered clipboard send and the clipboard is not empty
                    let eventclipboard = tunnel::EventClipboard {
                        data: data.to_owned(),
                    };
                    let clipboard_msg = tunnel::MessageClient {
                        msg: Some(tunnel::message_client::Msg::Clipboard(eventclipboard)),
                    };
                    events.push(clipboard_msg);
                }
                self.clipbard_trig = false;
            }
        }

        Ok(tunnel::MessagesClient { msgs: events })
    }

    fn display_stats(&self) -> bool {
        self.display_stats
    }

    fn printfile(&self, file: &str) -> Result<()> {
        info!("Print file {:?}", file);
        Ok(())
    }
}
