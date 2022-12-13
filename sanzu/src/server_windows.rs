use crate::{
    config::ConfigServer,
    server_utils::Server,
    utils::{ArgumentsSrv, ServerEvent},
    utils_win,
    video_encoder::{Encoder, EncoderTimings},
};
use anyhow::{Context, Result};
use byteorder::{LittleEndian, ReadBytesExt};

use clipboard_win::{formats, get_clipboard, set_clipboard};
use lock_keys::LockKeyWrapper;
use sanzu_common::tunnel;

use std::{
    cmp::Ordering,
    collections::HashMap,
    ffi::CString,
    io::Cursor,
    ptr::null_mut,
    sync::{
        atomic,
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
        dxgi, dxgi1_2, dxgitype,
        guiddef::GUID,
        minwindef::{BOOL, DWORD, LPARAM, LRESULT, UINT, WPARAM},
        windef::{HWND, RECT},
        winerror::{DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, SUCCEEDED},
    },
    um::{
        d3d11, d3dcommon,
        fileapi::{CreateFileA, OPEN_EXISTING},
        handleapi::INVALID_HANDLE_VALUE,
        ioapiset::DeviceIoControl,
        libloaderapi::GetModuleHandleA,
        setupapi::{
            SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiEnumDeviceInterfaces,
            SetupDiGetClassDevsA, SetupDiGetDeviceInterfaceDetailA,
            SetupDiGetDeviceRegistryPropertyA, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SPDRP_ADDRESS,
            SPDRP_BUSNUMBER, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_A,
            SP_DEVINFO_DATA,
        },
        wingdi::{GetDeviceCaps, HORZRES, VERTRES},
        winioctl::{CTL_CODE, FILE_ANY_ACCESS, FILE_DEVICE_UNKNOWN, METHOD_BUFFERED},
        winuser::{
            CreateWindowExA, DefWindowProcA, DispatchMessageA, EnumWindows, GetDC,
            GetSystemMetrics, GetWindowInfo, GetWindowLongA, GetWindowRect, GetWindowTextA,
            GetWindowTextLengthA, IsIconic, IsWindowVisible, PeekMessageA, RegisterClassExA,
            SendInput, SetClipboardViewer, TranslateMessage, GWL_EXSTYLE, INPUT, INPUT_KEYBOARD,
            INPUT_MOUSE, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE,
            MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
            MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
            MOUSEEVENTF_WHEEL, MSG, PM_REMOVE, SM_CXSIZEFRAME, WINDOWINFO, WM_DRAWCLIPBOARD,
            WM_KILLFOCUS, WM_QUIT, WM_SETFOCUS, WNDCLASSEXA, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
            WS_DLGFRAME, WS_EX_TOOLWINDOW, WS_POPUP,
        },
    },
};

lazy_static! {
    // TODO XXX: how to share handle?
    // This variables are initialized once and won't be changed.
    static ref WINHANDLE: Mutex<u64> = Mutex::new(0);
    static ref EVENT_SENDER: Mutex<Option<Sender<tunnel::MessageSrv>>> = Mutex::new(None);

}

const IVSHMEM_CACHE_WRITECOMBINED: u8 = 2;

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
    /// For ivshmem export
    pub map_info: Option<(u64, u64)>,
}

#[derive(Debug)]
pub struct Area {
    pub drawable: usize,
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

lazy_static! {
    static ref P_DIRECT3D_DEVICE: atomic::AtomicPtr<d3d11::ID3D11Device> =
        atomic::AtomicPtr::new(null_mut());
    static ref P_DIRECT3D_DEVICE_CONTEXT: atomic::AtomicPtr<d3d11::ID3D11DeviceContext> =
        atomic::AtomicPtr::new(null_mut());
    static ref P_OUTPUTDUPLICATION: atomic::AtomicPtr<dxgi1_2::IDXGIOutputDuplication> =
        atomic::AtomicPtr::new(null_mut());
    static ref P_TEXTURE2D: atomic::AtomicPtr<d3d11::ID3D11Texture2D> =
        atomic::AtomicPtr::new(null_mut());
    static ref P_SURFACE: atomic::AtomicPtr<dxgi::IDXGISurface> =
        atomic::AtomicPtr::new(null_mut());
    static ref SURFACE_MAP_ADDR: atomic::AtomicPtr<u8> = atomic::AtomicPtr::new(null_mut());
    static ref SURFACE_MAP_PITCH: atomic::AtomicI32 = atomic::AtomicI32::new(0);
    static ref AREAS: Mutex<HashMap<usize, Area>> = Mutex::new(HashMap::new());
}

fn bool_to_key_state(state: bool) -> lock_keys::LockKeyState {
    match state {
        true => lock_keys::LockKeyState::Enabled,
        false => lock_keys::LockKeyState::Disabled,
    }
}

extern "system" fn custom_wnd_proc(
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
            if let Ok(data) = get_clipboard(formats::Unicode) {
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

extern "system" fn enum_window_callback(hwnd: HWND, _l_param: LPARAM) -> BOOL {
    let is_visible = unsafe { IsWindowVisible(hwnd) };

    // Skip if windows is not visible
    if is_visible == 0 {
        return 1;
    }

    let ex_style = unsafe { GetWindowLongA(hwnd, GWL_EXSTYLE) } as u32;

    // Skip if windows a tool window
    if ex_style & WS_EX_TOOLWINDOW != 0 {
        return 1;
    }

    let is_iconic = unsafe { IsIconic(hwnd) };

    // Skip if windows is iconic
    if is_iconic != 0 {
        return 1;
    }

    let text_length = unsafe { GetWindowTextLengthA(hwnd) };
    let mut buffer = vec![0u8; text_length as usize + 1];
    unsafe {
        GetWindowTextA(hwnd, buffer.as_mut_ptr() as *mut i8, text_length + 1);
    };
    let title = String::from_utf8_lossy(&buffer);
    let is_filtered = title == "Windows Shell Experience Host\0" || title == "\0";

    // HACK: Skip if windows is Shell experience Host or has no title
    if is_filtered {
        return 1;
    }

    let mut rect_win = RECT::default();

    let _ = unsafe { GetWindowRect(hwnd, &mut rect_win) };

    let (win_x, win_y) = (rect_win.left, rect_win.top);
    let win_w = rect_win.right - rect_win.left;
    let win_h = rect_win.bottom - rect_win.top;

    let mut win_info = WINDOWINFO::default();

    let _ = unsafe { GetWindowInfo(hwnd, &mut win_info) };

    let border_thickness = unsafe { GetSystemMetrics(SM_CXSIZEFRAME) };

    // Remove border (left, right, bottom)
    // Also add 1 pixel to the border to get window state rectangle
    let win_w = win_w - (border_thickness as i32) * 2 + 2;
    let win_h = win_h - border_thickness as i32 + 1;
    let win_x = win_x + (border_thickness as i32) - 1;

    let area = Area {
        drawable: hwnd as usize,
        position: (win_x as i16, win_y as i16),
        size: (win_w as u16, win_h as u16),
        mapped: true,
    };

    AREAS.lock().unwrap().insert(hwnd as usize, area);

    1
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

// https://github.com/apitrace/apitrace/blob/master/lib/guids/guids_entries.h

fn d3d11_get_device_idxgidevice(device: &d3d11::ID3D11Device) -> Result<&dxgi::IDXGIDevice> {
    /* GUID_ENTRY(0x54ec77fa,0x1377,0x44e6,0x8c,0x32,0x88,0xfd,0x5f,0x44,0xc8,0x4c,IID_IDXGIDevice) */
    let riid_idxgidevice = GUID {
        Data1: 0x54ec77fa,
        Data2: 0x1377,
        Data3: 0x44e6,
        Data4: [0x8c, 0x32, 0x88, 0xfd, 0x5f, 0x44, 0xc8, 0x4c],
    };

    let mut p_idxgidevice: *mut c_void = null_mut();

    let ret = unsafe {
        device.QueryInterface(&riid_idxgidevice as *const _, &mut p_idxgidevice as *mut _)
    };

    if SUCCEEDED(ret) {
        info!("Query interface success {:?}", p_idxgidevice);
    } else {
        error!("Error in queryinterface");
        return Err(anyhow!("Error in query interface idxgidevice"));
    }
    let p_idxgidevice: *mut dxgi::IDXGIDevice = unsafe { std::mem::transmute(p_idxgidevice) };
    unsafe { p_idxgidevice.as_ref() }.context("Null idxgiadapter")
}

fn d3d11_get_idxgidevice_idxgiadapter(
    idxgidevice: &dxgi::IDXGIDevice,
) -> Result<&dxgi::IDXGIAdapter> {
    /* GUID_ENTRY(0x2411e7e1,0x12ac,0x4ccf,0xbd,0x14,0x97,0x98,0xe8,0x53,0x4d,0xc0,IID_IDXGIAdapter) */
    let riid_idxgiadapter = GUID {
        Data1: 0x2411e7e1,
        Data2: 0x12ac,
        Data3: 0x4ccf,
        Data4: [0xbd, 0x14, 0x97, 0x98, 0xe8, 0x53, 0x4d, 0xc0],
    };

    let mut p_idxgiadapter: *mut c_void = null_mut();

    let ret = unsafe {
        idxgidevice.GetParent(
            &riid_idxgiadapter as *const _,
            &mut p_idxgiadapter as *mut _,
        )
    };

    if !SUCCEEDED(ret) {
        error!("Error in get parent idxgiadapter");
        return Err(anyhow!("Error in get parent idxgiadapter"));
    }
    let p_idxgiadapter: *mut dxgi::IDXGIAdapter = unsafe { std::mem::transmute(p_idxgiadapter) };
    unsafe { p_idxgiadapter.as_ref() }.context("Null idxgiadapter")
}

fn d3d11_get_idxgioutput_idxgioutput1(
    idxgioutput: &dxgi::IDXGIOutput,
) -> Result<&dxgi1_2::IDXGIOutput1> {
    /* DEFINE_GUID(IID_IDXGIOutput1,0x00cddea8,0x939b,0x4b83,0xa3,0x40,0xa6,0x85,0x22,0x66,0x66,0xcc);  */
    let riid_idxgioutput1 = GUID {
        Data1: 0x00cddea8,
        Data2: 0x939b,
        Data3: 0x4b83,
        Data4: [0xa3, 0x40, 0xa6, 0x85, 0x22, 0x66, 0x66, 0xcc],
    };

    let mut p_idxgioutput1: *mut c_void = null_mut();

    let ret = unsafe {
        idxgioutput.QueryInterface(
            &riid_idxgioutput1 as *const _,
            &mut p_idxgioutput1 as *mut _,
        )
    };

    if !SUCCEEDED(ret) {
        error!("Error in queryinterface idxgioutput1");
        return Err(anyhow!("Error in get parent idxgioutput1"));
    }
    let p_idxgioutput1: *mut dxgi1_2::IDXGIOutput1 = unsafe { std::mem::transmute(p_idxgioutput1) };
    unsafe { p_idxgioutput1.as_ref() }.context("Null idxgioutput1")
}

fn d3d11_get_resource_texture2d(resource: &dxgi::IDXGIResource) -> Result<&d3d11::ID3D11Texture2D> {
    /* GUID_ENTRY(0x6f15aaf2,0xd208,0x4e89,0x9a,0xb4,0x48,0x95,0x35,0xd3,0x4f,0x9c,IID_ID3D11Texture2D) */
    let riid_id3d11texture2d = GUID {
        Data1: 0x6f15aaf2,
        Data2: 0xd208,
        Data3: 0x4e89,
        Data4: [0x9a, 0xb4, 0x48, 0x95, 0x35, 0xd3, 0x4f, 0x9c],
    };

    let mut p_id3d11texture2d: *mut c_void = null_mut();

    let ret = unsafe {
        resource.QueryInterface(
            &riid_id3d11texture2d as *const _,
            &mut p_id3d11texture2d as *mut _,
        )
    };

    if !SUCCEEDED(ret) {
        error!("Error in queryinterface id3d11texture2d");
        return Err(anyhow!("Error in get parent id3d11texture2d"));
    }
    let p_id3d11texture2d: *mut d3d11::ID3D11Texture2D =
        unsafe { std::mem::transmute(p_id3d11texture2d) };
    unsafe { p_id3d11texture2d.as_ref() }.context("Null id3d11texture2d")
}

fn d3d11_get_texture2d_surface(texture2d: &d3d11::ID3D11Texture2D) -> Result<&dxgi::IDXGISurface> {
    /* GUID_ENTRY(0xcafcb56c,0x6ac3,0x4889,0xbf,0x47,0x9e,0x23,0xbb,0xd2,0x60,0xec,IID_IDXGISurface) */
    let riid_idxgisurface = GUID {
        Data1: 0xcafcb56c,
        Data2: 0x6ac3,
        Data3: 0x4889,
        Data4: [0xbf, 0x47, 0x9e, 0x23, 0xbb, 0xd2, 0x60, 0xec],
    };

    let mut p_idxgisurface: *mut c_void = null_mut();

    let ret = unsafe {
        texture2d.QueryInterface(
            &riid_idxgisurface as *const _,
            &mut p_idxgisurface as *mut _,
        )
    };

    if !SUCCEEDED(ret) {
        error!("Error in queryinterface idxgisurface");
        return Err(anyhow!("Error in get parent idxgisurface"));
    }
    let p_idxgisurface: *mut dxgi::IDXGISurface = unsafe { std::mem::transmute(p_idxgisurface) };
    unsafe { p_idxgisurface.as_ref() }.context("Null idxgisurface")
}

/// Acquire dxgi frame
/// Re init d3d11 if desktop has been lost
fn acquire_dxgi_frame() -> Result<(Vec<u8>, u32, u32, u32)> {
    debug!("acquire img");
    let idxgioutputduplication =
        unsafe { P_OUTPUTDUPLICATION.load(atomic::Ordering::Acquire).as_ref() }
            .context("Null outputduplication")?;

    let mut frame_info = dxgi1_2::DXGI_OUTDUPL_FRAME_INFO::default();
    let mut p_desktop_resource: *mut dxgi::IDXGIResource = null_mut();
    let ret = unsafe {
        idxgioutputduplication.AcquireNextFrame(
            0,
            &mut frame_info,
            &mut p_desktop_resource as *mut _,
        )
    };
    if !SUCCEEDED(ret) {
        warn!("acquirenextframe {:?}", ret);
        let result = match ret {
            DXGI_ERROR_ACCESS_LOST => {
                // Re init d3d11
                if let Err(err) = init_d3d11().context("Cannot init d3d11") {
                    err.chain().for_each(|cause| error!(" - due to {}", cause));
                }
                Err(anyhow!("Access lost"))
            }
            DXGI_ERROR_WAIT_TIMEOUT => Err(anyhow!("wait timeout")),
            err => Err(anyhow!(format!("Unknown err {:?}", err))),
        };
        return result;
    }
    debug!("frame acquired!");

    if frame_info.PointerShapeBufferSize != 0 {
        debug!("pointer size {}", frame_info.PointerShapeBufferSize);
        let mut pointer = vec![0u8; frame_info.PointerShapeBufferSize as usize];
        let mut buffer_size_required = 0;
        let mut shape_info = dxgi1_2::DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
        let ret = unsafe {
            idxgioutputduplication.GetFramePointerShape(
                frame_info.PointerShapeBufferSize,
                pointer.as_mut_ptr() as *mut c_void,
                &mut buffer_size_required,
                &mut shape_info,
            )
        };
        if ret == 0 {
            debug!(
                "pointer info {:?} {}x{}",
                shape_info.Type, shape_info.Width, shape_info.Height
            );
            if shape_info.Type == dxgi1_2::DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR {
                let cursor_event = tunnel::EventCursor {
                    data: pointer,
                    width: shape_info.Width as u32,
                    height: shape_info.Height as u32,
                    xhot: shape_info.HotSpot.x as u32,
                    yhot: shape_info.HotSpot.y as u32,
                };
                let msg_cursor = tunnel::MessageSrv {
                    msg: Some(tunnel::message_srv::Msg::Cursor(cursor_event)),
                };
                EVENT_SENDER
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .send(msg_cursor)
                    .expect("Cannot send event cursor");
            } else {
                error!("Unsupported pointer type {:?}", shape_info.Type);
            }
        }
    }

    let desktop_resource =
        unsafe { p_desktop_resource.as_ref() }.context("p_desktop_resource is null")?;

    let acquireddesktopimage =
        d3d11_get_resource_texture2d(desktop_resource).context("Cannot get desktop resource")?;
    let p_acquireddesktopimage: *mut d3d11::ID3D11Texture2D = acquireddesktopimage
        as *const winapi::um::d3d11::ID3D11Texture2D
        as *mut winapi::um::d3d11::ID3D11Texture2D;

    let p_id3d11texture2d = P_TEXTURE2D.load(atomic::Ordering::Acquire);

    let p_copy_resource: *mut d3d11::ID3D11Resource =
        unsafe { std::mem::transmute(p_id3d11texture2d) };

    let d3d11_device_context = unsafe {
        P_DIRECT3D_DEVICE_CONTEXT
            .load(atomic::Ordering::Acquire)
            .as_ref()
    }
    .context("Device context null")?;

    unsafe {
        d3d11_device_context
            .CopyResource(p_copy_resource as *mut _, p_acquireddesktopimage as *mut _)
    };

    let ret = unsafe { idxgioutputduplication.ReleaseFrame() };
    if !SUCCEEDED(ret) {
        panic!("cannot release frame");
    }
    debug!("Copy done");

    let surface =
        unsafe { P_SURFACE.load(atomic::Ordering::Acquire).as_ref() }.context("Null surface")?;

    debug!("Surface ok");

    let mut surface_desc = dxgi::DXGI_SURFACE_DESC::default();
    let ret = unsafe { surface.GetDesc(&mut surface_desc) };
    if !SUCCEEDED(ret) {
        panic!("cannot desc surface");
    }

    debug!(
        "surface {:?}x{:?} format {:?}",
        surface_desc.Width, surface_desc.Height, surface_desc.Format,
    );

    let map = dxgi::DXGI_MAPPED_RECT {
        Pitch: SURFACE_MAP_PITCH.load(atomic::Ordering::Acquire),
        pBits: SURFACE_MAP_ADDR.load(atomic::Ordering::Acquire),
    };

    debug!("data {:?}", map.pBits);
    let data = unsafe {
        std::slice::from_raw_parts(
            map.pBits as *mut u8,
            (map.Pitch * surface_desc.Height as i32) as usize,
        )
    };
    debug!("data {:?}", &data[0..10]);

    let data = data.to_owned();

    Ok((
        data,
        surface_desc.Width,
        surface_desc.Height,
        map.Pitch as u32,
    ))
}

/// # Safety
///
/// Initialise Direct3D by calling unsafe Windows API
pub fn init_d3d11() -> Result<()> {
    let driver_types = vec![
        d3dcommon::D3D_DRIVER_TYPE_HARDWARE,
        d3dcommon::D3D_DRIVER_TYPE_WARP,
        d3dcommon::D3D_DRIVER_TYPE_REFERENCE,
    ];

    let feature_levels = vec![
        d3dcommon::D3D_FEATURE_LEVEL_11_0,
        d3dcommon::D3D_FEATURE_LEVEL_10_1,
        d3dcommon::D3D_FEATURE_LEVEL_10_0,
        d3dcommon::D3D_FEATURE_LEVEL_9_1,
    ];
    let num_feature_levels = feature_levels.len() as u32;

    let mut p_d3d11_device: *mut d3d11::ID3D11Device = null_mut();
    let mut feature_level: d3dcommon::D3D_FEATURE_LEVEL = 0;
    let mut p_d3d11_device_context: *mut d3d11::ID3D11DeviceContext = null_mut();

    for driver_type in driver_types {
        info!("Create D3D11 device for {}", driver_type);
        let ret = unsafe {
            d3d11::D3D11CreateDevice(
                null_mut(),
                driver_type,
                null_mut(),
                d3d11::D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                feature_levels.as_ptr() as *const _,
                num_feature_levels,
                d3d11::D3D11_SDK_VERSION,
                &mut p_d3d11_device as *mut _,
                &mut feature_level,
                &mut p_d3d11_device_context as *mut _,
            )
        };
        if !SUCCEEDED(ret) {
            continue;
        }
        P_DIRECT3D_DEVICE.store(p_d3d11_device, atomic::Ordering::Release);
        P_DIRECT3D_DEVICE_CONTEXT.store(p_d3d11_device_context, atomic::Ordering::Release);
        let d3d11_device = unsafe { p_d3d11_device.as_ref() }.context("d3d11 device null")?;

        info!("D3D11 device ok for {} {:?}", driver_type, ret);
        info!(
            "D3D11 {:?} {:?} {:?}",
            p_d3d11_device, feature_level, p_d3d11_device_context
        );
        info!("known features level {:?}", feature_levels);

        let idxgidevice =
            d3d11_get_device_idxgidevice(d3d11_device).context("Cannot get idxgidevice")?;

        let idxgiadapter =
            d3d11_get_idxgidevice_idxgiadapter(idxgidevice).context("Cannot get idxgiadapter")?;

        let mut index = 0;
        let mut monitor_infos = vec![];
        loop {
            let mut p_idxgioutput: *mut dxgi::IDXGIOutput = null_mut();
            let ret = unsafe { idxgiadapter.EnumOutputs(index, &mut p_idxgioutput as *mut _) };
            index += 1;
            if !SUCCEEDED(ret) {
                break;
            }
            let idxgioutput = if let Some(idxgioutput) = unsafe { p_idxgioutput.as_ref() } {
                idxgioutput
            } else {
                continue;
            };

            let mut desktopdesc = dxgi::DXGI_OUTPUT_DESC::default();
            let ret = unsafe { idxgioutput.GetDesc(&mut desktopdesc as *mut _) };
            if !SUCCEEDED(ret) {
                continue;
            }
            monitor_infos.push(desktopdesc);
            let idxgioutput1 = d3d11_get_idxgioutput_idxgioutput1(idxgioutput)
                .context("Cannot get idxgioutput1")?;

            let mut p_idxgioutputduplication: *mut dxgi1_2::IDXGIOutputDuplication = null_mut();

            let ret = unsafe {
                idxgioutput1.DuplicateOutput(
                    p_d3d11_device as *mut _,
                    &mut p_idxgioutputduplication as *mut _,
                )
            };
            if !SUCCEEDED(ret) {
                error!("Cannot dup output {:x}", ret);
                continue;
            }
            P_OUTPUTDUPLICATION.store(p_idxgioutputduplication, atomic::Ordering::Release);

            let idxgioutputduplication = unsafe { p_idxgioutputduplication.as_ref() }
                .context("Null idxgioutputduplication")?;

            let mut output_desc = dxgi1_2::DXGI_OUTDUPL_DESC::default();
            unsafe { idxgioutputduplication.GetDesc(&mut output_desc as *mut _) };
            info!(
                "mode desc {:?}x{:?} refreshrate {:?}/{:?} format {:?} scanlineorder {:?} scaling {:?}",
                output_desc.ModeDesc.Width,
                output_desc.ModeDesc.Height,
                output_desc.ModeDesc.RefreshRate.Numerator,
                output_desc.ModeDesc.RefreshRate.Denominator,
                output_desc.ModeDesc.Format,
                output_desc.ModeDesc.ScanlineOrdering,
                output_desc.ModeDesc.Scaling,
            );
            info!("rotation {:?}", output_desc.Rotation);
            info!(
                "desktop in systemmem {:?}",
                output_desc.DesktopImageInSystemMemory
            );

            let sample_desc = dxgitype::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            };

            let desc = d3d11::D3D11_TEXTURE2D_DESC {
                Width: output_desc.ModeDesc.Width,
                Height: output_desc.ModeDesc.Height,
                MipLevels: 1,
                ArraySize: 1,
                Format: output_desc.ModeDesc.Format,
                SampleDesc: sample_desc,
                Usage: d3d11::D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: d3d11::D3D11_CPU_ACCESS_READ | d3d11::D3D11_CPU_ACCESS_WRITE,
                MiscFlags: 0,
            };

            let mut p_id3d11texture2d: *mut d3d11::ID3D11Texture2D = null_mut();
            let ret = unsafe {
                d3d11_device.CreateTexture2D(&desc, null_mut(), &mut p_id3d11texture2d as *mut _)
            };
            if !SUCCEEDED(ret) {
                return Err(anyhow!("Cannot create texture2d"));
            }

            P_TEXTURE2D.store(p_id3d11texture2d, atomic::Ordering::Release);

            let id3d11texture2d =
                unsafe { p_id3d11texture2d.as_ref() }.context("p_desktop_resource is null")?;

            let idxgisurface = d3d11_get_texture2d_surface(id3d11texture2d)
                .context("Cannot get surface interface")?;

            let p_idxgisurface: *mut dxgi::IDXGISurface = idxgisurface
                as *const winapi::shared::dxgi::IDXGISurface
                as *mut winapi::shared::dxgi::IDXGISurface;

            P_SURFACE.store(p_idxgisurface, atomic::Ordering::Release);
            let surface = unsafe { p_idxgisurface.as_ref() }.context("Null idxgisurface")?;

            let mut map = dxgi::DXGI_MAPPED_RECT::default();
            let ret = unsafe { surface.Map(&mut map, dxgi::DXGI_MAP_READ) };
            if !SUCCEEDED(ret) {
                panic!("cannot map surface");
            }
            SURFACE_MAP_ADDR.store(map.pBits, atomic::Ordering::Release);
            SURFACE_MAP_PITCH.store(map.Pitch, atomic::Ordering::Release);

            return Ok(());
        }
        for monitor in monitor_infos {
            info!("Monitor!");
            info!(
                "Info name {:?} AttachedToDesktop {:?} Rotation {:?} Monitor {:?}",
                monitor.DeviceName, monitor.AttachedToDesktop, monitor.Rotation, monitor.Monitor,
            );
        }

        break;
    }

    Err(anyhow!("Cannot create d3d11 device"))
}

pub fn map_ivshmem() -> Result<(u64, u64)> {
    let ioctl_ivshmem_request_size: DWORD =
        CTL_CODE(FILE_DEVICE_UNKNOWN, 0x801, METHOD_BUFFERED, FILE_ANY_ACCESS);
    let ioctl_ivshmem_request_mmap: DWORD =
        CTL_CODE(FILE_DEVICE_UNKNOWN, 0x802, METHOD_BUFFERED, FILE_ANY_ACCESS);

    let guid_devinterface_ivshmem = GUID {
        Data1: 0xdf576976,
        Data2: 0x569d,
        Data3: 0x4672,
        Data4: [0x95, 0xa0, 0xf5, 0x7e, 0x4e, 0xa0, 0xb2, 0x10],
    };

    let dev_info_set = unsafe {
        SetupDiGetClassDevsA(
            &guid_devinterface_ivshmem,
            null_mut(),
            null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if dev_info_set == INVALID_HANDLE_VALUE {
        panic!("Error during SetupDiGetClassDevsA");
    }

    for index in 0.. {
        let mut dev_info_data = SP_DEVINFO_DATA::default();
        dev_info_data.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
        let ret = unsafe { SetupDiEnumDeviceInfo(dev_info_set, index, &mut dev_info_data) };
        if ret == 0 {
            continue;
        }

        /* Read bus */
        let mut bus_data = vec![0u8; 4];
        let ret = unsafe {
            SetupDiGetDeviceRegistryPropertyA(
                dev_info_set,
                &mut dev_info_data,
                SPDRP_BUSNUMBER,
                null_mut(),
                bus_data.as_mut_ptr() as *mut u8,
                std::mem::size_of::<DWORD>() as u32,
                null_mut(),
            )
        };

        if ret == 0 {
            continue;
        }

        let mut rdr = Cursor::new(bus_data);
        let bus = match rdr.read_u32::<LittleEndian>() {
            Ok(bus) => bus,
            Err(_) => {
                continue;
            }
        };

        /* Read addr */
        let mut addr_data = vec![0u8; 4];
        let ret = unsafe {
            SetupDiGetDeviceRegistryPropertyA(
                dev_info_set,
                &mut dev_info_data,
                SPDRP_ADDRESS,
                null_mut(),
                addr_data.as_mut_ptr() as *mut u8,
                std::mem::size_of::<DWORD>() as u32,
                null_mut(),
            )
        };

        if ret == 0 {
            continue;
        }

        let mut rdr = Cursor::new(addr_data);
        let addr = match rdr.read_u32::<LittleEndian>() {
            Ok(addr) => addr,
            Err(_) => {
                continue;
            }
        };

        info!("IVSHMEM: {} {:x}", bus, addr);

        let mut dev_interface_data = SP_DEVICE_INTERFACE_DATA::default();
        dev_interface_data.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;

        let ret = unsafe {
            SetupDiEnumDeviceInterfaces(
                dev_info_set,
                &mut dev_info_data,
                &guid_devinterface_ivshmem,
                0,
                &mut dev_interface_data,
            )
        };

        if ret == 0 {
            warn!("Error in SetupDiEnumDeviceInterfaces");
            continue;
        }

        let mut req_size: DWORD = 0;

        unsafe {
            SetupDiGetDeviceInterfaceDetailA(
                dev_info_set,
                &mut dev_interface_data,
                null_mut(),
                0,
                &mut req_size,
                null_mut(),
            )
        };
        info!("Req size {:x}", req_size);

        if req_size == 0 {
            warn!("Error in SetupDiGetDeviceInterfaceDetail req size");
            continue;
        }

        info!("Req size {:x}", req_size);

        let mut inf_data_buffer = vec![0u8; req_size as usize];
        let p_inf_data: *mut SP_DEVICE_INTERFACE_DETAIL_DATA_A =
            unsafe { std::mem::transmute(inf_data_buffer.as_mut_ptr()) };
        let mut inf_data = unsafe { p_inf_data.as_mut() }.context("Null inf_data")?;
        inf_data.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_A>() as u32;
        info!("inf data {:x}", inf_data.cbSize);
        let ret = unsafe {
            SetupDiGetDeviceInterfaceDetailA(
                dev_info_set,
                &mut dev_interface_data,
                p_inf_data,
                req_size,
                null_mut(),
                null_mut(),
            )
        };

        if ret == 0 {
            warn!("Error in SetupDiGetDeviceInterfaceDetail for device");
            continue;
        }

        info!("IVSHMEM path: {:?}", inf_data.DevicePath);

        let handle = unsafe {
            CreateFileA(
                inf_data.DevicePath.as_mut_ptr(),
                0,
                0,
                null_mut(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };

        if handle == INVALID_HANDLE_VALUE {
            unsafe {
                SetupDiDestroyDeviceInfoList(dev_info_set);
            }
            warn!("CreateFile returned INVALID_HANDLE_VALUE");
            continue;
        }

        info!("Open ivshmem ok");

        let mut size_data = vec![0u8; 8];
        let ret = unsafe {
            DeviceIoControl(
                handle,
                ioctl_ivshmem_request_size,
                null_mut(),
                0,
                size_data.as_mut_ptr() as *mut c_void,
                std::mem::size_of::<u64>() as u32,
                null_mut(),
                null_mut(),
            )
        };
        if ret == 0 {
            warn!("DeviceIoControl Failed");
            continue;
        }

        let mut rdr = Cursor::new(size_data);
        let size = match rdr.read_u64::<LittleEndian>() {
            Ok(size) => size,
            Err(_) => {
                continue;
            }
        };
        info!("ivhsmem size: {:x}\n", size);

        #[repr(C)]
        #[derive(Debug)]
        struct IvshmemMmap {
            peer_id: u16,
            size: u64,
            ptr: *mut c_void,
            vectors: u16,
        }

        #[repr(C)]
        #[derive(Debug)]
        struct IvshmemMmapConfig {
            cache_mode: u8,
        }

        let ivshmem_mmap_size = std::mem::size_of::<IvshmemMmap>();
        let ivshmem_mmap_config_size = std::mem::size_of::<IvshmemMmapConfig>();

        let mut map_buffer = vec![0u8; ivshmem_mmap_size];
        let p_map: *mut IvshmemMmap = unsafe { std::mem::transmute(map_buffer.as_mut_ptr()) };
        let map = unsafe { p_map.as_mut() }.context("Null map")?;

        let mut config_buffer = vec![0u8; ivshmem_mmap_config_size];
        let p_config: *mut IvshmemMmapConfig =
            unsafe { std::mem::transmute(config_buffer.as_mut_ptr()) };
        let mut config = unsafe { p_config.as_mut() }.context("Null config")?;
        config.cache_mode = IVSHMEM_CACHE_WRITECOMBINED;

        let ret = unsafe {
            DeviceIoControl(
                handle,
                ioctl_ivshmem_request_mmap,
                p_config as *mut c_void,
                ivshmem_mmap_config_size as u32,
                p_map as *mut c_void,
                ivshmem_mmap_size as u32,
                null_mut(),
                null_mut(),
            )
        };
        if ret == 0 {
            warn!("DeviceIoControl Failed");
            continue;
        }
        info!("mmap: {:x?} {:x}\n", map.ptr, map.size);
        return Ok((map.ptr as u64, map.size));
    }

    Err(anyhow!("No ivshmem found"))
}

pub fn init_win(
    arguments: &ArgumentsSrv,
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
                WS_POPUP | WS_DLGFRAME | WS_CLIPSIBLINGS | WS_CLIPCHILDREN,
                0,
                0,
                10,
                10,
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
    init_d3d11().context("Cannot init d3d11")?;

    let mut map_info = None;
    if arguments.export_video_pci {
        info!("Search ivshmem");
        map_info = Some(map_ivshmem().context("Error during ivshmem")?);
    }

    info!("ivshmem result: {:?}", map_info);

    let server = ServerInfo {
        img: None,
        max_stall_img: config.video.max_stall_img,
        frozen_frames_count: 0,
        img_count: 0,
        width: screen_width,
        height: screen_height,
        event_receiver,
        map_info,
    };
    Ok(Box::new(server))
}

impl Server for ServerInfo {
    fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    fn grab_frame(&mut self) -> Result<()> {
        let time_ac1 = Instant::now();
        let (data, width, height, pitch) = match acquire_dxgi_frame() {
            Err(err) => {
                err.chain().for_each(|cause| error!(" - due to {}", cause));
                return Ok(());
            }
            Ok(x) => x,
        };
        let time_ac2 = Instant::now();
        info!("duration: {:?}", time_ac2 - time_ac1);

        // invert image
        debug!("img {}x{} {}", width, height, pitch);
        let mut data_sized = vec![0u8; data.len()];
        let bpp = (width * 4) as usize;
        for index in 0..height as usize {
            data_sized[index * bpp..index * bpp + bpp]
                .copy_from_slice(&data[index * pitch as usize..index * pitch as usize + bpp]);
        }
        self.img = Some(data_sized);
        drop(data);
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
                    /* Reset frames count to send image with fresh resolution */
                    self.frozen_frames_count = 0;
                    server_events.push(ServerEvent::ResolutionChange(event.width, event.height));
                }
                Some(tunnel::message_client::Msg::Clipboard(event)) => {
                    info!("Clipboard retrieved from client");
                    set_clipboard(formats::Unicode, event.data.clone())
                        .map_err(|err| anyhow!("Err {:?}", err))
                        .context("Cannot set clipboard")?;
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
            }
        }
        Ok(server_events)
    }

    fn poll_events(&mut self) -> Result<Vec<tunnel::MessageSrv>> {
        let mut events = vec![];
        while let Ok(event) = self.event_receiver.try_recv() {
            events.push(event);
        }

        AREAS.lock().unwrap().clear();

        let _ = unsafe { EnumWindows(Some(enum_window_callback), 0) };

        for (index, area) in AREAS.lock().unwrap().iter() {
            let area_new = tunnel::EventAreaUpdt {
                id: *index as u32,
                x: area.position.0 as i32,
                y: area.position.1 as i32,
                width: area.size.0 as u32,
                height: area.size.1 as u32,
                mapped: area.mapped,
                is_app: true,
                name: "".to_string(),
            };
            let event_area_updt = tunnel::message_srv::Msg::AreaUpdt(area_new);
            let event_area_updt = tunnel::MessageSrv {
                msg: Some(event_area_updt),
            };
            events.push(event_area_updt);
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
                if let Some(ref mut map_info) = &mut self.map_info {
                    let mem_out = unsafe {
                        std::slice::from_raw_parts_mut(map_info.0 as *mut u8, map_info.1 as _)
                    };
                    mem_out[..data.len()].copy_from_slice(data);
                }

                let (width, height) = (self.width as u32, self.height as u32);
                let result = video_encoder
                    .encode_image(data, width, height, width * 4, self.img_count)
                    .context("Error in encode image")?;

                let encoded = result.0;
                timings = Some(result.1);

                /* Prepare encoded image */
                let img = match video_encoder.is_raw() {
                    true => match &self.map_info {
                        None => tunnel::message_srv::Msg::ImgRaw(tunnel::ImageRaw {
                            data: encoded,
                            width,
                            height,
                            bytes_per_line: width * 4,
                        }),
                        Some(_) => tunnel::message_srv::Msg::ImgRaw(tunnel::ImageRaw {
                            data: vec![],
                            width,
                            height,
                            bytes_per_line: width * 4,
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
        error!("Change resolution: unsupported os");
        Ok(())
    }

    fn activate_window(&self, _win_id: u32) -> Result<()> {
        Ok(())
    }
}
