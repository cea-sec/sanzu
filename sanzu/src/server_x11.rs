use crate::{
    config::ConfigServer,
    server_utils::Server,
    utils::ClipboardSelection,
    utils::{get_xwd_data, ArgumentsSrv, ServerEvent},
    utils_x11,
    video_encoder::{Encoder, EncoderTimings},
};
use anyhow::{Context, Result};
use byteorder::{ByteOrder, LittleEndian, ReadBytesExt};
#[cfg(feature = "notify")]
use dbus::channel::MatchingReceiver;

use libc::{self, shmat, shmctl, shmdt, shmget};
use memmap2::{Mmap, MmapMut};

use sanzu_common::tunnel;

use std::{
    cmp::Ordering,
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{Cursor, Write},
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
    rust_connection::{DefaultStream, RustConnection},
    COPY_DEPTH_FROM_PARENT,
};

/// Holds information on a server side window.
///
/// TODO: for now, we only support rectangle windows.
#[derive(Debug)]
pub struct Area {
    pub drawable: Window,
    pub position: (i16, i16),
    pub size: (u16, u16),
    pub mapped: bool,
}

impl Eq for Area {}

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
        self.mapped.cmp(&other.mapped)
    }
}

impl PartialEq for Area {
    fn eq(&self, other: &Self) -> bool {
        self.drawable == other.drawable
            && self.position == other.position
            && self.size == other.size
            && self.mapped == other.mapped
    }
}

impl PartialOrd for Area {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        let ret = self.drawable.partial_cmp(&other.drawable);
        if ret != Some(Ordering::Equal) {
            return ret;
        }

        let ret = self.size.partial_cmp(&other.size);
        if ret != Some(Ordering::Equal) {
            return ret;
        }
        let ret = self.position.partial_cmp(&other.position);
        if ret != Some(Ordering::Equal) {
            return ret;
        }
        self.mapped.partial_cmp(&other.mapped)
    }
}

fn get_window_childs<C: Connection>(conn: &C, window: Window) -> Result<Vec<Window>> {
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
        if let Ok(children) = get_window_childs(conn, parent) {
            for child in children {
                debug!("  child {:?} parent {:?}", child, parent);
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
        width as u16,
        height as u16,
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
                .context(format!("Error in open {:?}", extern_img_source_path))?;
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
        width: width as u16,
        height: height as u16,
        addr,
        export_video_mmap,
        extern_img_source_mmap,
    })
}

/// Creates Area linked to a `window`
pub fn init_area<C: Connection>(conn: &C, window: Window, root: Window) -> Result<Area> {
    let geometry = conn
        .get_geometry(window)
        .context("Error in get geometry")?
        .reply()
        .context("Error in get geometry reply")?;
    let atom_string_hidden = conn
        .intern_atom(false, b"_NET_WM_STATE_HIDDEN")
        .context("Error in intern_atom")?
        .reply()
        .context("Error in intern_atom reply")?;
    let atom_hidden_a: Atom = atom_string_hidden.atom;
    let mut mapped = true;
    if let Ok(client_list) = get_client_list(conn, root) {
        if let Ok(children) = get_window_childs(conn, window) {
            for child in children {
                if client_list.contains(&child) {
                    if let Ok(states) = get_window_state(conn, child) {
                        if states.contains(&atom_hidden_a) {
                            mapped = false;
                        }
                    }
                }
            }
        }
    }
    Ok(Area {
        drawable: window,
        position: (geometry.x, geometry.y),
        size: (geometry.width, geometry.height),
        mapped,
    })
}

/// Holds information on the server
pub struct ServerX11 {
    /// x11 connection handle
    pub conn: RustConnection<DefaultStream>,
    /// x11 graphic information
    pub grabinfo: GrabInfo,
    /// Frame rate limit (see config)
    pub max_stall_img: u32,
    /// Areas handled by the server
    pub areas: HashMap<usize, Area>,
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

/// Retrieve the x11 windows information
pub fn get_client_list<C: Connection>(conn: &C, root: Window) -> Result<Vec<Window>> {
    let atom_string_target = conn
        .intern_atom(false, b"_NET_CLIENT_LIST")
        .context("Error in intern_atom")?
        .reply()
        .context("Error in intern_atom reply")?;
    let atom_property_a: Atom = atom_string_target.atom;
    let ret = conn
        .get_property(false, root, atom_property_a, AtomEnum::WINDOW, 0, 0xFFFF)
        .context("Error in get_property")?
        .reply()
        .context("Error in get_property reply")?;

    let mut windows = vec![];
    for data in ret.value.chunks(4) {
        let value: &[u8] = data;
        let mut rdr = Cursor::new(value);
        let window: u32 = rdr
            .read_u32::<LittleEndian>()
            .context("Error in read_u32")?;
        windows.push(window as Window);
    }
    Ok(windows)
}

pub fn get_window_state<C: Connection>(conn: &C, window: Window) -> Result<Vec<Window>> {
    let atom_string_target = conn
        .intern_atom(false, b"_NET_WM_STATE")
        .context("Error in intern_atom")?
        .reply()
        .context("Error in intern_atom reply")?;
    let atom_property_a: Atom = atom_string_target.atom;
    let ret = conn
        .get_property(false, window, atom_property_a, AtomEnum::ATOM, 0, 0xFFFF)
        .context("Error in get_property")?
        .reply()
        .context("Error in get_property reply")?;

    let mut states = vec![];
    for data in ret.value.chunks(4) {
        let value: &[u8] = data;
        let mut rdr = Cursor::new(value);
        let window: u32 = rdr
            .read_u32::<LittleEndian>()
            .context("Error in read_u32")?;
        states.push(window as Window);
    }
    Ok(states)
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
    let entries = fs::read_dir(PATH_PCI_DEVICES)
        .context(format!("Error in read dir {}", PATH_PCI_DEVICES))?;
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
                .context(format!("Error in open {:?}", path_resource))?;
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
        sleep(Duration::new(0, 100_000_000));
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

    conn.flush().context("Error in x11rb flush")?;

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
        arguments.extern_img_source.map(|path| path.to_string()),
        config,
        width,
        height,
    )
    .context("Error in init_grab")?;

    let mut areas = HashMap::new();

    let setup = conn.setup();
    let screen = &setup.roots[screen_num];
    let root = screen.root;

    let windows = get_client_list(&conn, root).context("Error in get_client_list")?;
    info!("Windows: {:?}", windows);

    let result = get_windows_parents(&conn, screen.root).context("Error in get_windows_parents")?;
    for (index, (target_root, target)) in result.into_iter().enumerate() {
        if !windows.contains(&target_root) {
            continue;
        }
        info!("Window found {:?} {:?}", target_root, target);

        let target_window = target;
        let area = init_area(&conn, target_window, root).context("Error in init_area")?;
        if area.size.0 > 1 && area.size.1 > 1 {
            areas.insert(index, area);
        }
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
    let video_mode_index = if utils_x11::get_video_mode(&conn, window_info, VIDEO_NAME_2)
        .context("Error in get_video_mode")?
        .is_some()
    {
        1
    } else {
        0
    };

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
    let extern_img_source = arguments.extern_img_source.map(|path| path.to_string());

    let server = ServerX11 {
        conn,
        max_stall_img: config.video.max_stall_img,
        areas,
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
fn create_area(server: &mut ServerX11, window: Window, root: Window) -> bool {
    if server.root == window {
        // Avoid root window
        return false;
    }
    let mut found = false;
    if let Ok(reply) = server.conn.get_window_attributes(window) {
        if let Ok(attributes) = reply.reply() {
            if attributes.map_state == MapState::from(0) {
                // Window is not mapped => skip it
                return false;
            }
        }
    }

    for (_index, area) in server.areas.iter() {
        if area.drawable == window {
            found = true;
        }
    }

    if found {
        return false;
    }

    if let Ok(area) = init_area(&server.conn, window, root) {
        if area.size.0 <= 1 || area.size.1 <= 1 {
            return false;
        }
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
    map: bool,
) -> bool {
    for (_, area) in server.areas.iter_mut() {
        trace!("Known window {:?} {:?}", area.drawable, area);
        if area.drawable == window {
            area.mapped = map;
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
                    trace!("{:?}", event);
                    if create_area(self, event.window, self.root) {
                        self.modified_area = true;
                    }
                }
                Event::MapNotify(event) => {
                    trace!("{:?}", event);
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
                        true,
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
        debug!("push areas");
        for (index, area) in self.areas.iter() {
            let area_new = tunnel::EventAreaUpdt {
                id: *index as u32,
                x: area.position.0 as i32,
                y: area.position.1 as i32,
                width: area.size.0 as u32,
                height: area.size.1 as u32,
                mapped: area.mapped,
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
        let custom_mode = utils_x11::add_video_mode(
            &self.conn,
            self.window,
            width as u16,
            height as u16,
            new_video_name,
            new_video_index,
        )
        .context("Error in add_video_mode")?;

        // Set video mode
        utils_x11::set_video_mode(&self.conn, self.window, custom_mode).map_err(|err| {
            if utils_x11::delete_video_mode_by_name(&self.conn, self.window, new_video_name)
                .is_err()
            {
                warn!("Error in delete_video_mode_by_name");
            }
            err.context("Cannot set mode")
        })?;

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
                if let Ok(children) = get_window_childs(&self.conn, area.drawable) {
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
