use crate::{
    config::ConfigServer,
    server_utils::Server,
    utils::ClipboardSelection,
    utils::{get_xwd_data, ArgumentsSrv, ServerEvent},
    utils_x11,
    video_encoder::{Encoder, EncoderTimings},
};
use anyhow::{Context, Result};
use byteorder::{ByteOrder, LittleEndian};
#[cfg(feature = "notify")]
use dbus::channel::MatchingReceiver;
use encoding_rs::mem::decode_latin1;

use libc::{self, shmat, shmctl, shmdt, shmget};
use lock_keys::LockKeyWrapper;
use memmap2::{Mmap, MmapMut};
use sanzu_common::tunnel;

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    ptr::null_mut,
    sync::{
        mpsc::{channel, Receiver},
        Arc, Mutex,
    },
    thread::{self, sleep},
    time::{Duration, Instant},
};

#[cfg(any(feature = "notify", feature = "printfile"))]
use std::sync::mpsc::Sender;

use utils_x11::{get_clipboard_events, listen_clipboard};

use x11_clipboard::Clipboard;

const PATH_PCI_DEVICES: &str = "/sys/bus/pci/devices/";

use x11rb::{
    connection::{Connection, RequestConnection},
    protocol::{
        damage::ConnectionExt as ConnectionExtXDamage,
        randr::{self, ConnectionExt as _},
        shm::{self, ConnectionExt as ConnectionExtShm},
        xfixes::{self, ConnectionExt as _},
        xproto::ConnectionExt as _,
        xproto::*,
        xtest::ConnectionExt as ConnectionExtXTest,
        Event,
    },
    rust_connection::RustConnection,
    COPY_DEPTH_FROM_PARENT,
};

/// Holds information on a server side window.
///
/// TODO: for now, we only support rectangle windows.
#[derive(Debug, PartialEq, Eq)]
pub struct Area {
    pub drawable: Window,
    pub position: (i16, i16),
    pub size: (u16, u16),
    pub mapped: bool,
    pub is_app: bool,
    pub name: String,
}

impl Ord for Area {
    fn cmp(&self, other: &Self) -> Ordering {
        let ret = self.drawable.cmp(&other.drawable);
        if ret != Ordering::Equal {
            return ret;
        }

        let ret = self.size.cmp(&other.size);
        if ret != Ordering::Equal {
            return ret;
        }
        let ret = self.position.cmp(&other.position);
        if ret != Ordering::Equal {
            return ret;
        }
        let ret = self.mapped.cmp(&other.mapped);
        if ret != Ordering::Equal {
            return ret;
        }
        let ret = self.is_app.cmp(&other.is_app);
        if ret != Ordering::Equal {
            return ret;
        }
        self.name.cmp(&other.name)
    }
}

impl PartialOrd for Area {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn get_window_children<C: Connection>(conn: &C, window: Window) -> Result<Vec<Window>> {
    // Find children
    let response = conn
        .query_tree(window)
        .context("Error in query_tree")?
        .reply()
        .context("Error in query_tree reply")?;
    let mut out = vec![];

    for child in response.children {
        out.push(child);
    }

    Ok(out)
}

fn get_windows_parents<C: Connection>(conn: &C, window: Window) -> Result<HashMap<Window, Window>> {
    let response = conn
        .query_tree(window)
        .context("Error in query tree")?
        .reply()
        .context("Error in query tree reply")?;

    let mut out = HashMap::new();

    for parent in response.children {
        if let Ok(children) = get_window_children(conn, parent) {
            for child in children {
                debug!("  child {:x} parent {:x}", child, parent);
                out.insert(child, parent);
            }
        } else {
            warn!("Unknown name, skipping");
            continue;
        };
    }

    Ok(out)
}

/// Holds information on the server graphics
#[derive(Debug)]
pub struct GrabInfo {
    /// x11 handle
    pub drawable: u32,
    /// Screen size
    pub size: usize,
    /// x11 shmseg
    pub shmseg: u32,
    pub width: u16,
    pub height: u16,
    /// x11 shared memory raw pointer
    pub addr: *const u8,
    /// address of the pci video export
    pub export_video_mmap: Option<MmapMut>,
    pub extern_img_source_mmap: Option<Mmap>,
}

fn init_grab<C: Connection>(
    conn: &C,
    screen: &Screen,
    export_video_pci: bool,
    extern_img_source: Option<String>,
    config: &ConfigServer,
    width: u16,
    height: u16,
) -> Result<GrabInfo> {
    let drawable = screen.root;

    let size = width as usize * height as usize * 4;
    let shmseg = conn.generate_id().context("Error in x11rb generate_id")?;
    debug!("shmget ok");

    // Video is exported via tcp / vsock
    let shmid = unsafe {
        let ret = shmget(libc::IPC_PRIVATE, size, libc::IPC_CREAT | 0o777);
        if ret < 0 {
            return Err(anyhow!("Error in shmget"));
        }
        ret
    };
    conn.shm_attach(shmseg, shmid as u32, false)
        .context("Error in shm attach")?;
    conn.flush().context("Error in x11rb flush")?;
    debug!("shm attach ok");

    conn.shm_get_image(
        drawable,
        0,
        0,
        width,
        height,
        0xFFFFFFFF,
        ImageFormat::Z_PIXMAP.into(),
        shmseg,
        0,
    )
    .context("Error in shm get image")?
    .reply()
    .context("Error in shm get image reply")?;
    debug!("shm_get_image ok");

    let addr = unsafe { shmat(shmid, null_mut(), 0) } as *const u8;
    debug!("shm addr {:?}", addr);
    let ptr_bad = usize::MAX as *const u8;
    if addr == ptr_bad {
        return Err(anyhow!("ShmAt Error"));
    }

    if unsafe { shmctl(shmid, libc::IPC_RMID, null_mut()) } != 0 {
        return Err(anyhow!("shmctl error"));
    }

    let export_video_mmap = match (export_video_pci, &config.export_video_pci) {
        (true, Some(ref export_video_pci)) => {
            // Video is exported wia pci shared mem
            let shared_mem_file =
                find_pci_shared_memory(&export_video_pci.device, &export_video_pci.vendor)
                    .context("Cannot find pci")?;

            let shared_mem_mmap = unsafe {
                MmapMut::map_mut(&shared_mem_file).context("Cannot map memory video file")?
            };
            Some(shared_mem_mmap)
        }
        (false, _) => None,
        _ => {
            return Err(anyhow!("Bad export video configuration"));
        }
    };

    let extern_img_source_mmap = match extern_img_source {
        Some(extern_img_source_path) => {
            let file = OpenOptions::new()
                .read(true)
                .create(false)
                .open(&extern_img_source_path)
                .context(format!("Error in open {extern_img_source_path:?}"))?;
            let extern_img_source =
                unsafe { Mmap::map(&file).context("Cannot map extern video source")? };
            Some(extern_img_source)
        }
        None => None,
    };

    Ok(GrabInfo {
        drawable,
        size,
        shmseg,
        width,
        height,
        addr,
        export_video_mmap,
        extern_img_source_mmap,
    })
}

/// Creates Area linked to a `window`
pub fn init_area<C: Connection>(conn: &C, root: Window, window: Window) -> Result<Area> {
    let geometry = conn
        .get_geometry(window)
        .context("Error in get geometry")?
        .reply()
        .context("Error in get geometry reply")?;
    let mut mapped = false;
    if let Ok(reply) = conn.get_window_attributes(window) {
        if let Ok(attributes) = reply.reply() {
            if attributes.map_state == MapState::VIEWABLE {
                mapped = true;
            }
        }
    }
    let app_list = get_client_list(conn, root).context("Error in get_client_list")?;

    trace!("init area {:x}", window);

    let mut app_name = "".to_string();
    let is_app = if let Ok(windows_children) = get_window_children(conn, window) {
        let mut found = false;

        for child in windows_children.iter() {
            if app_list.contains(child) {
                if let Ok(name) = get_window_name(conn, *child) {
                    app_name = name;
                }
                trace!("child {:x}: {}", *child, app_name);
                found = true;
                break;
            }
        }
        found
    } else {
        false
    };

    Ok(Area {
        drawable: window,
        position: (geometry.x, geometry.y),
        size: (geometry.width, geometry.height),
        mapped,
        is_app,
        name: app_name,
    })
}

/// Holds information on the server
pub struct ServerX11 {
    /// x11 connection handle
    pub conn: RustConnection,
    /// x11 graphic information
    pub grabinfo: GrabInfo,
    /// Frame rate limit (see config)
    pub max_stall_img: u32,
    /// Areas handled by the server
    pub areas: HashMap<usize, Area>,
    /// Windows app handles
    pub apps: Vec<Window>,
    /// Number of encoded frames
    pub img_count: i64,
    /// x11 screen index
    pub screen_num: usize,
    /// Current number of identical server frames
    pub frozen_frames_count: u32,
    /// Current graphic has changed
    pub modified_img: bool,
    /// Monitored areas have changed
    pub modified_area: bool,
    #[cfg(feature = "notify")]
    /// dbus handle
    pub dbus_conn: Option<dbus::blocking::Connection>,
    #[cfg(feature = "notify")]
    /// dbus events handle
    pub notifications_receiver: Receiver<Notifications>,
    /// Server clipboard handle
    pub clipboard: Clipboard,
    /// Server window
    pub window: Window,
    /// Server root window
    pub root: Window,
    /// Screen width
    pub width: u16,
    /// Screen height
    pub height: u16,
    /// Current video mode index
    pub video_mode_index: usize,
    /// Allow to send clipboard to client
    pub restrict_clipboard: bool,
    /// Clipboard event receiver
    pub clipboard_event_receiver: Receiver<String>,
    /// store clipboard events to skip
    pub skip_clipboard_primary: Arc<Mutex<u32>>,
    pub skip_clipboard_clipboard: Arc<Mutex<u32>>,
    pub extern_img_source: Option<String>,
    pub avoid_img_extraction: bool,
    #[cfg(feature = "printfile")]
    /// dbus printfile receiver
    pub dbus_printfile_receiver: Receiver<PrintFile>,
}

fn get_property32<C: Connection>(
    conn: &C,
    window: Window,
    property: &[u8],
    kind: impl Into<u32>,
) -> Result<Vec<u32>> {
    let atom_property = conn
        .intern_atom(false, property)
        .context("Error in intern_atom")?
        .reply()
        .context("Error in intern_atom reply")?
        .atom;
    let ret = conn
        .get_property(false, window, atom_property, kind.into(), 0, 0xFFFF)
        .context("Error in get_property")?
        .reply()
        .context("Error in get_property reply")?;

    let values = ret
        .value32()
        .context("Incorrect format in GetProperty reply")?;
    Ok(values.collect())
}

/// Retrieve the x11 windows information
pub fn get_client_list<C: Connection>(conn: &C, root: Window) -> Result<Vec<Window>> {
    get_property32(conn, root, b"_NET_CLIENT_LIST", AtomEnum::WINDOW)
}

pub fn get_window_state<C: Connection>(conn: &C, window: Window) -> Result<Vec<Window>> {
    get_property32(conn, window, b"_NET_WM_STATE", AtomEnum::ATOM)
}

pub fn get_window_name<C: Connection>(conn: &C, window: Window) -> Result<String> {
    let atom_string_target = conn
        .intern_atom(false, b"_NET_WM_NAME")
        .context("Error in intern_atom")?
        .reply()
        .context("Error in intern_atom reply")?;
    let atom_property_a: Atom = atom_string_target.atom;
    let ret = conn
        .get_property(false, window, atom_property_a, 0u32, 0, 0xFFFF)
        .context("Error in get_property")?
        .reply()
        .context("Error in get_property reply")?;
    let value = String::from_utf8(ret.value).unwrap_or_else(|e| decode_latin1(e.as_bytes()).into());
    Ok(value)
}

#[cfg(feature = "notify")]
/// Retrieve server notifications (messages and images)
///
/// For now, only filter events generated by Firefox.
fn notification_extract_message(msg: dbus::Message) -> Vec<Notification> {
    let mut notifications = vec![];
    let items = msg.get_items();
    let mut items_iter = items.iter();
    let app = items_iter.next();
    let _ = items_iter.next();
    let _ = items_iter.next();
    let title = items_iter.next();
    //let icon = None;
    if let (
        Some(dbus::arg::messageitem::MessageItem::Str(ref app)),
        Some(dbus::arg::messageitem::MessageItem::Str(ref title)),
    ) = (app, title)
    {
        if app == "Firefox" {
            notifications.push(Notification::Title(title.to_owned()));
            for item in items_iter {
                match item {
                    dbus::arg::messageitem::MessageItem::Str(ref string) => {
                        notifications.push(Notification::Message(string.to_owned()))
                    }
                    dbus::arg::messageitem::MessageItem::Dict(ref dict) => {
                        for (key, value) in dict.iter() {
                            if let (
                                dbus::arg::messageitem::MessageItem::Str(ref item_name),
                                dbus::arg::messageitem::MessageItem::Variant(ref item_value),
                            ) = (key, value)
                            {
                                if item_name == "icon_data" {
                                    if let dbus::arg::messageitem::MessageItem::Struct(
                                        ref icon_struct,
                                    ) = **item_value
                                    {
                                        if icon_struct.len() == 7 {
                                            let icon_width = &icon_struct[0];
                                            let icon_height = &icon_struct[1];
                                            //let icon_unk1 = &icon_struct[2];
                                            //let icon_unk2 = &icon_struct[3];
                                            let icon_bitsperpixel = &icon_struct[4];
                                            let icon_bytesperpixel = &icon_struct[5];
                                            let icon_data = &icon_struct[6];
                                            trace!(
                                                "icon {:?}x{:?} {:?} {:?}",
                                                icon_width,
                                                icon_height,
                                                icon_bitsperpixel,
                                                icon_bytesperpixel,
                                            );
                                            if let (
                                                dbus::arg::messageitem::MessageItem::Int32(
                                                    icon_width,
                                                ),
                                                dbus::arg::messageitem::MessageItem::Int32(
                                                    icon_height,
                                                ),
                                                dbus::arg::messageitem::MessageItem::Int32(
                                                    icon_bitsperpixel,
                                                ),
                                                dbus::arg::messageitem::MessageItem::Int32(
                                                    icon_bytesperpixel,
                                                ),
                                                dbus::arg::messageitem::MessageItem::Array(
                                                    icon_data,
                                                ),
                                            ) = (
                                                icon_width,
                                                icon_height,
                                                icon_bitsperpixel,
                                                icon_bytesperpixel,
                                                icon_data,
                                            ) {
                                                debug!(
                                                    "icon {:?}x{:?} {:?} {:?}",
                                                    icon_width,
                                                    icon_height,
                                                    icon_bitsperpixel,
                                                    icon_bytesperpixel,
                                                );
                                                let icon_data: Vec<u8> = icon_data
                                                    .iter()
                                                    .map(
                                                        |data| {
                                                            if let dbus::arg::messageitem::MessageItem::Byte(data) = data {
                                                                *data
                                                            } else {
                                                                0
                                                            }
                                                        }
                                                    )
                                                    .collect();

                                                if *icon_bitsperpixel == 8
                                                    && *icon_bytesperpixel == 4
                                                    && icon_data.len()
                                                        == *icon_width as usize
                                                            * *icon_height as usize
                                                            * 4
                                                {
                                                    notifications.push(Notification::Icon(
                                                        *icon_width as u32,
                                                        *icon_height as u32,
                                                        icon_data,
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    };
    notifications
}

#[cfg(feature = "notify")]
/// Create a dbus event monitor
fn connect_to_dbus(notif_sender: Sender<Notifications>) -> Result<dbus::blocking::Connection> {
    let dbus_conn = dbus::blocking::Connection::new_session().context("D-Bus connection failed")?;

    let rule = dbus::message::MatchRule::new();
    let proxy = dbus_conn.with_proxy(
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        Duration::from_millis(500),
    );
    proxy
        .method_call(
            "org.freedesktop.DBus.Monitoring",
            "BecomeMonitor",
            (vec![rule.match_str()], 0u32),
        )
        .context("Cannot become monitor")?;

    dbus_conn.start_receive(
        rule,
        Box::new(move |msg, _| {
            // TODO XXX we may want to filter special caracters ?
            let notifications = notification_extract_message(msg);
            let notifications = Notifications { notifications };
            notif_sender
                .send(notifications)
                .expect("Cannot send notification");
            true
        }),
    );
    Ok(dbus_conn)
}

#[cfg(feature = "printfile")]
/// Receive Printfile call and forward it to the server object
/// The request embeds the path of the file to print
/// This path will be used as hint on the client side to find the file to print
fn print_entry(
    _: &mut dbus_crossroads::Context,
    dbus_info: &mut DBusInfo,
    (path,): (String,),
) -> Result<(), dbus_crossroads::MethodErr> {
    debug!("Print {}", path);
    dbus_info
        .sender
        .send(PrintFile { path })
        .expect("Cannot send print file");
    Ok(())
}

#[cfg(feature = "printfile")]
/// Struct used in dbus method callback
struct DBusInfo {
    sender: Sender<PrintFile>,
}

#[cfg(feature = "printfile")]
/// Create the dbus print interface
/// Can be called with dbus-send:
/// dbus-send --type=method_call --dest=com.sanzu.dbus /print com.sanzu.dbus.print_entry string:"test"
fn connect_to_dbus_print(printfile_sender: Sender<PrintFile>) -> Result<()> {
    let c = dbus::blocking::Connection::new_session()?;
    c.request_name("com.sanzu.dbus", false, true, false)?;

    let mut cr = dbus_crossroads::Crossroads::new();
    let iface_token = cr.register("com.sanzu.dbus", |b| {
        b.method("print_entry", ("path",), (), print_entry);
    });
    let dbus_info = DBusInfo {
        sender: printfile_sender,
    };
    cr.insert("/print", &[iface_token], dbus_info);

    // Serve clients forever.
    cr.serve(&c)?;
    Ok(())
}

fn setup_window<C: Connection>(conn: &C, screen: &Screen) -> Result<Window> {
    let win_id = conn.generate_id().context("Error in x11rb generate_id")?;
    let win_aux = CreateWindowAux::new()
        .event_mask(EventMask::STRUCTURE_NOTIFY)
        .background_pixel(screen.white_pixel);

    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win_id,
        screen.root,
        0,
        0,
        10,
        10, // Windows needs a minimum size
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &win_aux,
    )
    .context("Error in create_window")?;

    Ok(win_id)
}

/// Find the PCI device which will be used to exfiltrate data to the host
fn find_pci_shared_memory(searched_device: &str, searched_vendor: &str) -> Result<fs::File> {
    let entries =
        fs::read_dir(PATH_PCI_DEVICES).context(format!("Error in read dir {PATH_PCI_DEVICES}"))?;
    for entry in entries {
        let dir = entry.context("Bad directory entry")?;
        let path = dir.path();
        let path_device = path.join("device");
        let path_vendor = path.join("subsystem_vendor");

        let device = fs::read_to_string(path_device).unwrap_or_else(|_| "".to_owned());
        let vendor = fs::read_to_string(path_vendor).unwrap_or_else(|_| "".to_owned());
        let device = device.trim_end();
        let vendor = vendor.trim_end();

        if device == searched_device && vendor == searched_vendor {
            let path_resource = path.join("resource2_wc");
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(false)
                .open(&path_resource)
                .context(format!("Error in open {path_resource:?}"))?;
            return Ok(file);
        }
    }
    Err(anyhow!("Cannot find PCI shared memory"))
}

const VIDEO_NAME_1: &str = "video_mode_1";
const VIDEO_NAME_2: &str = "video_mode_2";
const VIDEO_NAMES: &[&str; 2] = &[VIDEO_NAME_1, VIDEO_NAME_2];

fn del_custom_video_mode<C: Connection>(conn: &C, window: Window) -> Result<()> {
    if utils_x11::get_video_mode(conn, window, VIDEO_NAME_1)
        .context("Error in get video mode1")?
        .is_some()
    {
        utils_x11::delete_video_mode_by_name(conn, window, VIDEO_NAME_1)
            .context("Error in del video mode1")?;
    }
    if utils_x11::get_video_mode(conn, window, VIDEO_NAME_2)
        .context("Error in get video mode2")?
        .is_some()
    {
        utils_x11::delete_video_mode_by_name(conn, window, VIDEO_NAME_2)
            .context("Error in del video mode2")?;
    }
    Ok(())
}

/// Initialize x11rb server handler
pub fn init_x11rb(
    arguments: &ArgumentsSrv,
    config: &ConfigServer,
    server_size: Option<(u16, u16)>,
) -> Result<Box<dyn Server>> {
    let start = Instant::now();
    let (conn, screen_num) = loop {
        if Instant::now() - start > Duration::new(2, 0) {
            break Err(anyhow!("Time out connecting to X11 display"));
        }
        if let Ok((conn, screen_num)) = x11rb::rust_connection::RustConnection::connect(None)
            .map_err(|err| {
                warn!("Attempt to connect to X11 server failed: {}", err);
                err
            })
        {
            break Ok((conn, screen_num));
        }
        sleep(Duration::from_millis(100));
    }?;

    conn.extension_information(shm::X11_EXTENSION_NAME)
        .context("Error in get shm extension")?
        .context("Shm must be supported")?;

    conn.xfixes_query_version(100, 0)
        .context("Error in query xfixes version")?
        .reply()
        .context("Error in xfixes version reply")?;

    let setup = conn.setup();
    let screen = &setup.roots[screen_num];

    /* Randr */

    /* randr extension to detect screen resolution changes */
    conn.extension_information(randr::X11_EXTENSION_NAME)
        .context("failed to get extension information")?
        .context("Randr must be supported")?;

    conn.randr_select_input(screen.root, randr::NotifyMask::CRTC_CHANGE)
        .context("Error in randr select input")?
        .check()
        .context("Error in randr select input check")?;
    conn.xfixes_select_cursor_input(screen.root, xfixes::CursorNotifyMask::DISPLAY_CURSOR)
        .context("Error in xfixes select cursor input")?
        .check()
        .context("Error in xfixes select cursor input check")?;

    /* Register Damage events */
    conn.damage_query_version(10, 10)
        .context("Error in query damage version")?
        .reply()
        .context("Error in damage version reply")?;

    let damage = conn.generate_id().context("Error in generate_id")?;
    conn.damage_create(
        damage,
        screen.root,
        x11rb::protocol::damage::ReportLevel::RAW_RECTANGLES,
    )
    .context("Error in damage create")?
    .check()
    .context("Error in damage create check")?;

    conn.damage_subtract(damage, 0u32, 0u32)
        .context("Error in damage substract")?;

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

    let grabinfo = init_grab(
        &conn,
        screen,
        arguments.export_video_pci,
        arguments.extern_img_source.clone(),
        config,
        width,
        height,
    )
    .context("Error in init_grab")?;

    let mut areas = HashMap::new();
    let mut known_windows = HashSet::new();

    let setup = conn.setup();
    let screen = &setup.roots[screen_num];
    let root = screen.root;

    /* Add WM windows */
    let app_list = get_client_list(&conn, root).context("Error in get_client_list")?;
    debug!(
        "Windows: {:?} Root: {:x}",
        app_list
            .iter()
            .map(|window| format!("{window:x}"))
            .collect::<Vec<String>>(),
        root
    );

    let result = get_windows_parents(&conn, screen.root).context("Error in get_windows_parents")?;
    let mut index = 0;
    for (target_root, target) in result.into_iter() {
        if !app_list.contains(&target_root) {
            continue;
        }
        trace!("Window found {:?} {:?}", target_root, target);

        let target_window = target;
        if known_windows.contains(&target_window) {
            continue;
        }
        let area = init_area(&conn, root, target_window).context("Error in init_area")?;
        if area.size.0 > 1 && area.size.1 > 1 {
            known_windows.insert(area.drawable);
            areas.insert(index, area);
            index += 1;
        }
    }

    /* Add root's chidren windows */
    let children = get_window_children(&conn, root).context("Cannot get root children")?;
    debug!("ROOT Windows: {}", children.len());
    for window in children {
        if known_windows.contains(&window) {
            continue;
        }
        if let Ok(reply) = conn.get_window_attributes(window) {
            if let Ok(attributes) = reply.reply() {
                trace!("    Attr: {:?}", attributes);
                if attributes.map_state == MapState::VIEWABLE && attributes.map_is_installed {
                    let area = init_area(&conn, root, window).context("Error in init_area")?;
                    known_windows.insert(area.drawable);
                    areas.insert(index, area);
                    index += 1;
                }
            }
        }
    }

    for (id, area) in areas.iter() {
        trace!("    {}:{:?}", id, area);
    }

    /* Register window structure modifications */
    let prop = ChangeWindowAttributesAux::default()
        .event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_NOTIFY);
    conn.change_window_attributes(screen.root, &prop)
        .context("Error in change_window_attributes")?
        .check()
        .context("Error in change_window_attributes check")?;

    /* Dbus for handling notification */
    #[cfg(feature = "notify")]
    let (dbus_conn, dbus_notif_receiver) = {
        let (dbus_notif_sender, dbus_notif_receiver) = channel();
        let dbus_conn = match connect_to_dbus(dbus_notif_sender) {
            Err(err) => {
                err.context("Cannot open dbus")
                    .chain()
                    .for_each(|cause| error!(" - due to {}", cause));
                None
            }
            Ok(dbus_conn) => Some(dbus_conn),
        };
        (dbus_conn, dbus_notif_receiver)
    };

    /* Dbus for handling print event */
    #[cfg(feature = "printfile")]
    let dbus_printfile_receiver = {
        let (dbus_printfile_sender, dbus_printfile_receiver) = channel();
        thread::spawn(move || {
            connect_to_dbus_print(dbus_printfile_sender).expect("Error in connect print");
        });
        dbus_printfile_receiver
    };

    /* Generate clipboard */
    let clipboard = Clipboard::new().context("Error in create clipboard")?;

    let window_info = setup_window(&conn, screen).context("Error in setup_window")?;

    let width = grabinfo.width;
    let height = grabinfo.height;

    del_custom_video_mode(&conn, window_info).context("Error in del_custom_video_mode")?;
    // If we are already stucked in custom mode, update index accordingly
    let video_mode_index = usize::from(
        utils_x11::get_video_mode(&conn, window_info, VIDEO_NAME_2)
            .context("Error in get_video_mode")?
            .is_some(),
    );

    let (selection_sender_primary, clipboard_event_receiver) = channel();
    let selection_sender_clipboard = selection_sender_primary.clone();

    let skip_clipboard_primary = Arc::new(Mutex::new(0));
    let skip_clipboard_clipboard = Arc::new(Mutex::new(0));

    let skip_clipboard_primary_thread = skip_clipboard_primary.clone();
    let skip_clipboard_clipboard_thread = skip_clipboard_clipboard.clone();

    if !arguments.restrict_clipboard {
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
    let extern_img_source = arguments.extern_img_source.clone();

    let server = ServerX11 {
        conn,
        max_stall_img: config.video.max_stall_img,
        areas,
        apps: app_list,
        grabinfo,
        img_count: 0,
        screen_num,
        frozen_frames_count: 0,
        modified_img: true,
        modified_area: true,
        #[cfg(feature = "notify")]
        dbus_conn,
        #[cfg(feature = "notify")]
        notifications_receiver: dbus_notif_receiver,
        clipboard,
        window: window_info,
        root,
        width,
        height,
        video_mode_index,
        restrict_clipboard: arguments.restrict_clipboard,
        clipboard_event_receiver,
        skip_clipboard_primary,
        skip_clipboard_clipboard,
        extern_img_source,
        avoid_img_extraction: arguments.avoid_img_extraction,
        #[cfg(feature = "printfile")]
        dbus_printfile_receiver,
    };

    Ok(Box::new(server))
}

/// Holds notifications messages
#[derive(Debug)]
pub enum Notification {
    Title(String),
    Message(String),
    Icon(u32, u32, Vec<u8>),
}

/// Holds notifications information
#[derive(Debug)]
pub struct Notifications {
    pub notifications: Vec<Notification>,
}

#[cfg(feature = "printfile")]
/// Holds printfile messages
#[derive(Debug)]
pub struct PrintFile {
    path: String,
}

/// Reparent known windows togethers, delete son
fn reparent_window(server: &mut ServerX11, window: Window, parent: Window) -> bool {
    trace!("Reparent");
    let mut found_window = None;
    for (index, area) in server.areas.iter() {
        if area.drawable == window {
            trace!("Window known {:?}", window);
            found_window = Some(*index);
            break;
        }
    }
    let mut found_parent = None;
    for (index, area) in server.areas.iter() {
        if area.drawable == parent {
            trace!("Parent known {:?}", parent);
            found_parent = Some(*index);
            break;
        }
    }
    if let (Some(index_window), Some(_index_parent)) = (found_window, found_parent) {
        server.areas.remove(&index_window);
        return true;
    }
    false
}

/// Create and link an area to a window
fn create_area(server: &mut ServerX11, root: Window, window: Window) -> bool {
    if server.root == window {
        // Avoid root window
        return false;
    }
    let mut found = false;

    for (_index, area) in server.areas.iter() {
        if area.drawable == window {
            found = true;
        }
    }

    if found {
        return false;
    }

    if let Ok(area) = init_area(&server.conn, root, window) {
        // find first free id, insert area
        for index in 0.. {
            if server.areas.get(&index).is_none() {
                server.areas.insert(index, area);
                return true;
            }
        }
    }
    false
}

fn destroy_area(server: &mut ServerX11, window: Window) -> bool {
    let mut found = None;
    for (index, area) in server.areas.iter() {
        trace!("Known window {:?} {:?}", area.drawable, area);
        if area.drawable == window {
            found = Some(*index);
        }
    }
    if let Some(index) = found {
        server.areas.remove(&index);
        return true;
    }
    false
}

fn map_area(server: &mut ServerX11, window: Window, map: bool) -> bool {
    let mut found = None;
    for (index, area) in server.areas.iter() {
        trace!("Known window {:?} {:?}", area.drawable, area);
        if area.drawable == window {
            found = Some(*index);
        }
    }
    if let Some(index) = found {
        let area = server.areas.get_mut(&index).expect("Cannot get area");
        area.mapped = map;
        return true;
    }
    false
}

fn update_area(
    server: &mut ServerX11,
    window: Window,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
) -> bool {
    for (_, area) in server.areas.iter_mut() {
        trace!("Known window {:?} {:?}", area.drawable, area);
        if area.drawable == window {
            area.position = (x, y);
            area.size = (width, height);
            return true;
        }
    }
    false
}

pub fn set_clipboard(server: &mut ServerX11, data: &str) -> Result<()> {
    /* Set *both* clipboards (primary and clipboard) */
    *server.skip_clipboard_clipboard.lock().unwrap() += 1;
    utils_x11::set_clipboard(&server.clipboard, 0, data).context("Error in set_clipboard")?;

    *server.skip_clipboard_primary.lock().unwrap() += 1;
    utils_x11::set_clipboard(&server.clipboard, 1, data).context("Error in set_clipboard")?;

    server.conn.flush().context("Error in x11rb flush")?;

    Ok(())
}

fn bool_to_key_state(state: bool) -> lock_keys::LockKeyState {
    match state {
        true => lock_keys::LockKeyState::Enabled,
        false => lock_keys::LockKeyState::Disabled,
    }
}

impl Server for ServerX11 {
    fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    fn grab_frame(&mut self) -> Result<()> {
        if self.extern_img_source.is_none() {
            self.conn
                .shm_get_image(
                    self.grabinfo.drawable,
                    0,
                    0,
                    self.grabinfo.width,
                    self.grabinfo.height,
                    0xFFFFFFFF,
                    ImageFormat::Z_PIXMAP.into(),
                    self.grabinfo.shmseg,
                    0,
                )
                .context("Error in shm_get_image")?
                .reply()
                .context("Error in shm_get_image reply")?;

            if let Err(err) = self.conn.flush() {
                error!("Connection error {:?}", err);
                return Err(anyhow!("Connection error"));
            }
        }

        Ok(())
    }

    fn handle_client_event(&mut self, msgs: tunnel::MessagesClient) -> Result<Vec<ServerEvent>> {
        let mut server_events = vec![];
        for msg in msgs.msgs.iter() {
            match &msg.msg {
                Some(tunnel::message_client::Msg::Move(event)) => {
                    trace!("Mouse move {} {}", event.x, event.y);
                    if let Err(err) = self.conn.xtest_fake_input(
                        6,
                        0,
                        0,
                        self.root,
                        event.x as i16,
                        event.y as i16,
                        0,
                    ) {
                        error!("Cannot send mouse move event: {}", err);
                    };
                }
                Some(tunnel::message_client::Msg::Button(event)) => {
                    trace!(
                        "Mouse button {} {} {} {}",
                        event.x,
                        event.y,
                        event.button,
                        event.updown
                    );
                    let eventid = match event.updown {
                        true => 4,
                        false => 5,
                    };
                    if let Err(err) = self.conn.xtest_fake_input(
                        eventid,
                        event.button as u8,
                        0,
                        self.root,
                        event.x as i16,
                        event.y as i16,
                        0,
                    ) {
                        error!("Cannot send mouse button event: {}", err);
                    };
                }
                Some(tunnel::message_client::Msg::Key(event)) => {
                    trace!("Key {} {}", event.keycode, event.updown);
                    let eventid = match event.updown {
                        true => 2,
                        false => 3,
                    };
                    if let Err(err) = self.conn.xtest_fake_input(
                        eventid,
                        event.keycode as u8,
                        0,
                        self.root,
                        0,
                        0,
                        0,
                    ) {
                        error!("Cannot send key event: {}", err);
                    };
                }
                Some(tunnel::message_client::Msg::Clipboard(event)) => {
                    info!("Clipboard retrieved from client");
                    if set_clipboard(self, &event.data).is_err() {
                        error!("Cannot set clipboard");
                    }
                }
                Some(tunnel::message_client::Msg::Display(event)) => {
                    /* Reset frames count to send image with fresh resolution */
                    self.frozen_frames_count = 0;
                    server_events.push(ServerEvent::ResolutionChange(event.width, event.height));
                }

                Some(tunnel::message_client::Msg::Activate(event)) => {
                    debug!("Activate {:?}", event);
                    if self.activate_window(event.id).is_err() {
                        error!("Cannot activate window");
                    }
                }

                Some(tunnel::message_client::Msg::Keylocks(event)) => {
                    info!("keyboard state {:?}", event);
                    let caps_lock = bool_to_key_state(event.caps_lock);
                    let num_lock = bool_to_key_state(event.num_lock);
                    let scroll_lock = bool_to_key_state(event.scroll_lock);

                    let lockkey = lock_keys::LockKey::new();

                    lockkey
                        .set(lock_keys::LockKeys::CapitalLock, caps_lock)
                        .unwrap();
                    lockkey
                        .set(lock_keys::LockKeys::NumberLock, num_lock)
                        .unwrap();
                    lockkey
                        .set(lock_keys::LockKeys::ScrollingLock, scroll_lock)
                        .unwrap();
                }
                _ => {}
            };
        }
        Ok(server_events)
    }

    fn poll_events(&mut self) -> Result<Vec<tunnel::MessageSrv>> {
        self.img_count += 1;
        self.modified_img = false;
        self.modified_area = false;
        let mut last_clipboard = None;
        let mut events = vec![];

        self.conn.flush().context("Cannot flush")?;

        while let Some(event) = self
            .conn
            .poll_for_event()
            .context("Error in poll_for_event")?
        {
            match event {
                Event::XfixesCursorNotify(event) => {
                    trace!("Cursor changer: {:?}", event);
                    let cursor = match xfixes::get_cursor_image(&self.conn) {
                        Ok(cursor) => cursor,
                        Err(err) => {
                            error!("Error in get cursor image: {}", err);
                            continue;
                        }
                    }
                    .reply()
                    .context("Error in get_cursor_image reply")?;
                    let mut cursor_data = vec![];
                    for data in cursor.cursor_image.iter() {
                        let mut pixel = vec![0u8; 4];
                        LittleEndian::write_u32(&mut pixel, *data);
                        cursor_data.append(&mut pixel);
                    }
                    let cursor_event = tunnel::EventCursor {
                        data: cursor_data,
                        width: cursor.width as u32,
                        height: cursor.height as u32,
                        xhot: cursor.xhot as u32,
                        yhot: cursor.yhot as u32,
                    };
                    let msg_cursor = tunnel::MessageSrv {
                        msg: Some(tunnel::message_srv::Msg::Cursor(cursor_event)),
                    };
                    events.push(msg_cursor);
                }
                Event::DamageNotify(event) => {
                    trace!("Damage: {:?}", event);
                    self.modified_img = true;
                }
                Event::NoExposure(_event) => {}

                Event::CreateNotify(event) => {
                    trace!("CreateNotify: {:?}", event);
                    if create_area(self, self.root, event.window) {
                        self.modified_area = true;
                    }
                }
                Event::MappingNotify(event) => {
                    trace!("{:?}", event);
                }
                Event::RandrNotify(event) => {
                    trace!("{:?}", event);
                }
                Event::MapNotify(event) => {
                    trace!("{:?}", event);
                    let mut found_window = None;
                    for (index, area) in self.areas.iter() {
                        if area.drawable == event.window {
                            trace!("Window known {:?}", event.window);
                            found_window = Some(*index);
                            break;
                        }
                    }
                    if found_window.is_none() {
                        debug!("Unknown window, adding");
                        if create_area(self, self.root, event.window) {
                            self.modified_area = true;
                        }
                    }
                    if map_area(self, event.window, true) {
                        self.modified_area = true;
                    }
                }
                Event::ReparentNotify(event) => {
                    trace!("{:?}", event);
                    if reparent_window(self, event.window, event.parent) {
                        self.modified_area = true;
                    } else {
                        warn!("Reparent strange");
                    }
                }
                Event::DestroyNotify(event) => {
                    trace!("{:?}", event);
                    if destroy_area(self, event.window) {
                        self.modified_area = true;
                    }
                }
                Event::UnmapNotify(event) => {
                    trace!("{:?}", event);
                    if map_area(self, event.window, false) {
                        self.modified_area = true;
                    }
                }
                Event::ConfigureNotify(event) => {
                    trace!("{:?}", event);
                    if update_area(
                        self,
                        event.window,
                        event.x,
                        event.y,
                        event.width,
                        event.height,
                    ) {
                        self.modified_area = true;
                    }
                }
                Event::ClientMessage(event) => {
                    trace!("{:?}", event);
                    self.modified_area = true;
                }
                Event::Error(_event) => {}
                _ => {
                    warn!("Unknown event {:?}", event);
                }
            }
        }

        /* Get clipboard events */
        if let Some(data) = get_clipboard_events(&self.clipboard_event_receiver) {
            let eventclipboard = tunnel::EventClipboard { data };
            last_clipboard = Some(tunnel::MessageSrv {
                msg: Some(tunnel::message_srv::Msg::Clipboard(eventclipboard)),
            });
        }

        if let Some(last_clipboard) = last_clipboard {
            events.push(last_clipboard);
        }

        if self.modified_img {
            self.frozen_frames_count = 0;
        } else {
            self.frozen_frames_count += 1;
        }
        /* Push areas infos */
        trace!("push areas");
        for (index, area) in self.areas.iter() {
            trace!("area {:x} {:?} {}", area.drawable, area.is_app, area.name);
            let area_new = tunnel::EventAreaUpdt {
                id: *index as u32,
                x: area.position.0 as i32,
                y: area.position.1 as i32,
                width: area.size.0 as u32,
                height: area.size.1 as u32,
                mapped: area.mapped,
                is_app: area.is_app,
                name: area.name.clone(),
            };
            let event_area_updt = tunnel::message_srv::Msg::AreaUpdt(area_new);
            let event_area_updt = tunnel::MessageSrv {
                msg: Some(event_area_updt),
            };
            events.push(event_area_updt);
        }

        // Get print file events
        #[cfg(feature = "printfile")]
        {
            while let Ok(printfile) = self.dbus_printfile_receiver.try_recv() {
                info!("Received printfile {:?}", printfile);
                let print = tunnel::EventPrintFile {
                    path: printfile.path,
                };
                let event_printfile = tunnel::MessageSrv {
                    msg: Some(tunnel::message_srv::Msg::Printfile(print)),
                };
                events.push(event_printfile);
            }
        }
        // Get notifications
        #[cfg(feature = "notify")]
        {
            self.dbus_conn
                .as_ref()
                .map(|dbus_conn| dbus_conn.process(Duration::from_millis(0)));
            while let Ok(notifications) = self.notifications_receiver.try_recv() {
                let mut notification_msgs = vec![];
                for notification in notifications.notifications {
                    match notification {
                        Notification::Title(string) => {
                            let notification = tunnel::Notification {
                                msg: Some(tunnel::notification::Msg::Title(string)),
                            };
                            notification_msgs.push(notification);
                        }
                        Notification::Message(string) => {
                            let notification = tunnel::Notification {
                                msg: Some(tunnel::notification::Msg::Message(string)),
                            };
                            notification_msgs.push(notification);
                        }
                        Notification::Icon(width, height, data) => {
                            let notification = tunnel::Notification {
                                msg: Some(tunnel::notification::Msg::Icon(
                                    tunnel::NotificationIcon {
                                        width,
                                        height,
                                        data,
                                    },
                                )),
                            };
                            notification_msgs.push(notification);
                        }
                    }
                }
                let notifications = tunnel::EventNotification {
                    notifications: notification_msgs,
                };
                let event_notif = tunnel::MessageSrv {
                    msg: Some(tunnel::message_srv::Msg::Notifications(notifications)),
                };
                events.push(event_notif);
            }
        }
        Ok(events)
    }

    fn generate_encoded_img(
        &mut self,
        video_encoder: &mut Box<dyn Encoder>,
    ) -> Result<(Vec<tunnel::MessageSrv>, Option<EncoderTimings>)> {
        let mut events = vec![];
        let mut timings = None;
        let (width, height) = (self.grabinfo.width as u32, self.grabinfo.height as u32);

        if self.frozen_frames_count > self.max_stall_img {
            trace!("Frozen img");
        } else if self.avoid_img_extraction {
            trace!("Avoid img extraction");
            let img = tunnel::message_srv::Msg::ImgRaw(tunnel::ImageRaw {
                data: vec![],
                width,
                height,
                bytes_per_line: width * 4,
            });
            let msg_img = tunnel::MessageSrv { msg: Some(img) };
            events.push(msg_img);
        } else {
            let (data, width, height, bytes_per_line) = if let Some(ref extern_img_source_mmap) =
                self.grabinfo.extern_img_source_mmap
            {
                // Grab frame from external xwd image
                let (data, _width, _height, bytes_per_line) = get_xwd_data(extern_img_source_mmap)?;
                (data, width, height, bytes_per_line)
            } else {
                // Grab from from x11 shm
                trace!("Grab from x11 {:?}", self.grabinfo.size);
                let data = unsafe {
                    std::slice::from_raw_parts(
                        self.grabinfo.addr as *mut u8,
                        self.grabinfo.size as _,
                    )
                };
                (data, width, height, width * 4)
            };
            trace!(
                "data len {} {}x{} bytes per line {}",
                data.len(),
                width,
                height,
                bytes_per_line
            );
            // If export to pci shared memory, sync it
            let mut time_memcpy = None;
            if let Some(ref mut mmap) = &mut self.grabinfo.export_video_mmap {
                trace!("Write to export video {:?}", data.len());
                let time_start = Instant::now();
                (&mut mmap[..])
                    .write_all(data)
                    .context("Error in write to video memory")?;
                mmap.flush_range(0, data.len())
                    .context("Cannot flush video memory")?;
                let time_stop = Instant::now();
                time_memcpy = Some(("memcpy", time_stop - time_start));
            }
            trace!("Encode");
            let result = video_encoder
                .encode_image(data, width, height, bytes_per_line, self.img_count)
                .unwrap();
            let encoded = result.0;
            let mut encoder_timings = result.1;
            if let Some(time_memcpy) = time_memcpy {
                encoder_timings.times.push(time_memcpy);
            }

            timings = Some(encoder_timings);

            /* Prepare encoded image */
            let img = match video_encoder.is_raw() {
                true => match &self.grabinfo.export_video_mmap {
                    None => tunnel::message_srv::Msg::ImgRaw(tunnel::ImageRaw {
                        data: encoded,
                        width,
                        height,
                        bytes_per_line,
                    }),
                    Some(_) => tunnel::message_srv::Msg::ImgRaw(tunnel::ImageRaw {
                        data: vec![],
                        width,
                        height,
                        bytes_per_line,
                    }),
                },
                false => tunnel::message_srv::Msg::ImgEncoded(tunnel::ImageEncoded {
                    data: encoded,
                    width,
                    height,
                }),
            };
            let msg_img = tunnel::MessageSrv { msg: Some(img) };
            events.push(msg_img);
        };

        Ok((events, timings))
    }

    fn change_resolution(&mut self, config: &ConfigServer, width: u32, height: u32) -> Result<()> {
        let (old_video_name, new_video_name, new_video_index) = if self.video_mode_index == 0 {
            (&VIDEO_NAMES[0], &VIDEO_NAMES[1], 1)
        } else {
            (&VIDEO_NAMES[1], &VIDEO_NAMES[0], 0)
        };

        // Add video mode
        trace!("Add video mode {:?} {}x{}", new_video_name, width, height);
        utils_x11::add_video_mode(
            &self.conn,
            self.window,
            width as u16,
            height as u16,
            new_video_name,
            new_video_index,
        )
        .context("Error in add_video_mode")?;

        // Create new grab info
        let setup = self.conn.setup();
        let screen = &setup.roots[self.screen_num];

        self.conn
            .shm_detach(self.grabinfo.shmseg)
            .context("Error in shm_detach")?;
        let ret = unsafe { shmdt(self.grabinfo.addr as *const std::ffi::c_void) };
        if ret != 0 {
            panic!("Cannot detach memory");
        }

        let grabinfo = init_grab(
            &self.conn,
            screen,
            self.grabinfo.export_video_mmap.is_some(),
            self.extern_img_source.clone(),
            config,
            width as u16,
            height as u16,
        )
        .context("Error in init_grab")?;
        self.grabinfo = grabinfo;

        // Delete old mode
        utils_x11::delete_video_mode_by_name(&self.conn, self.window, old_video_name)
            .context("Error in delete_video_mode_by_name")?;
        self.video_mode_index = new_video_index;

        Ok(())
    }

    fn activate_window(&self, win_id: u32) -> Result<()> {
        let atom_string_active = self
            .conn
            .intern_atom(false, b"_NET_ACTIVE_WINDOW")
            .context("Error in intern_atom")?
            .reply()
            .context("Error in intern_atom reply")?;
        let atom_active_a: Atom = atom_string_active.atom;

        if let Some(area) = self.areas.get(&(win_id as usize)) {
            if let Ok(client_list) = get_client_list(&self.conn, self.root) {
                if let Ok(children) = get_window_children(&self.conn, area.drawable) {
                    for child in children {
                        if client_list.contains(&child) {
                            let event = ClientMessageEvent {
                                response_type: CLIENT_MESSAGE_EVENT,
                                format: 32,
                                sequence: 0,
                                window: child,
                                type_: atom_active_a,
                                data: [0, 0, 0, 0, 0].into(),
                            };
                            self.conn.send_event(
                                false,
                                child,
                                EventMask::STRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
                                event,
                            )?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
