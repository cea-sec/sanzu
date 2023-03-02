use crate::{
    client_utils::{Area, Client},
    utils::{ArgumentsClient, ClipboardConfig},
    utils_win,
};
use anyhow::{Context, Result};

use clipboard_win::{formats, get_clipboard, set_clipboard};
use lock_keys::LockKeyWrapper;
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    ffi::CString,
    iter::FromIterator,
    ptr::null_mut,
    sync::{
        atomic,
        mpsc::{channel, sync_channel, Receiver, Sender, SyncSender},
        Mutex,
    },
    thread,
    time::Duration,
};

use sanzu_common::tunnel;

use spin_sleep::sleep;

use winapi::{
    ctypes::c_void,
    shared::{
        d3d9::{
            Direct3DCreate9, IDirect3D9, IDirect3DDevice9, IDirect3DSurface9, D3DADAPTER_DEFAULT,
            D3DCREATE_SOFTWARE_VERTEXPROCESSING, D3D_SDK_VERSION,
        },
        d3d9types::{
            D3DBACKBUFFER_TYPE_MONO, D3DCLEAR_TARGET, D3DCOLOR_XRGB, D3DDEVTYPE_HAL,
            D3DFMT_UNKNOWN, D3DFMT_X8R8G8B8, D3DLOCKED_RECT, D3DPOOL_DEFAULT,
            D3DPRESENT_PARAMETERS, D3DSWAPEFFECT_DISCARD, D3DTEXF_NONE,
        },
        minwindef::{DWORD, LPARAM, LRESULT, TRUE, UINT, WPARAM},
        windef::{HWND, HWND__, POINT, RECT},
    },
    um::{
        libloaderapi::{GetModuleHandleA, GetProcAddress, LoadLibraryA},
        processthreadsapi::ExitProcess,
        shellapi::ShellExecuteA,
        wingdi::{CombineRgn, ExtCreateRegion},
        wingdi::{
            CreateBitmap, CreateCompatibleBitmap, DeleteObject, RDH_RECTANGLES, RGNDATA,
            RGNDATAHEADER, RGN_OR,
        },
        winuser::{
            CreateIconIndirect, CreateWindowExA, DefWindowProcA, DestroyIcon, DestroyWindow,
            DispatchMessageA, GetCursorPos, GetDC, GetRawInputData, GetSystemMetrics, LoadImageA,
            PeekMessageA, PostQuitMessage, RegisterClassExA, RegisterRawInputDevices, ReleaseDC,
            SendMessageA, SetClipboardViewer, SetCursor, SetFocus, SetWindowRgn, SetWindowsHookExA,
            TranslateMessage, ICONINFO, ICON_BIG, IMAGE_ICON, LR_DEFAULTSIZE, LR_LOADFROMFILE, MSG,
            PM_REMOVE, PRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RIDEV_NOLEGACY,
            RID_INPUT, RIM_TYPEKEYBOARD, SM_CXSCREEN, SM_CYSCREEN, SW_HIDE, WH_MOUSE_LL,
            WM_ACTIVATE, WM_CHANGECBCHAIN, WM_CLOSE, WM_DESTROY, WM_DISPLAYCHANGE,
            WM_DRAWCLIPBOARD, WM_INPUT, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
            WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP,
            WM_SETFOCUS, WM_SETICON, WM_SIZE, WM_USER, WNDCLASSEXA, WS_CLIPCHILDREN,
            WS_CLIPSIBLINGS, WS_DLGFRAME, WS_MAXIMIZE, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_VISIBLE,
        },
    },
};

type FrameReceiver = Receiver<(Vec<u8>, u32, u32)>;
type CursorReceiver = Receiver<(Vec<u8>, u32, u32, i32, i32)>;

lazy_static! {
    static ref WINHANDLE: atomic::AtomicPtr<HWND__> = atomic::AtomicPtr::new(null_mut());
    static ref WIN_ID_TO_HANDLE: Mutex<HashMap<usize, u64>> = Mutex::new(HashMap::new());
    static ref HANDLE_TO_WIN_ID: Mutex<HashMap<u64, usize>> = Mutex::new(HashMap::new());
    static ref SCREEN_SIZE: Mutex<(u32, u32)> = Mutex::new((0, 0));
    static ref EVENT_SENDER: Mutex<Option<Sender<tunnel::MessageClient>>> = Mutex::new(None);
    static ref SESSION_SENDER: Mutex<Option<Sender<()>>> = Mutex::new(None);
    static ref CURSOR_RECEIVER: Mutex<Option<CursorReceiver>> = Mutex::new(None);
    static ref SHAPE_RECEIVER: Mutex<Option<Receiver<Vec<Area>>>> = Mutex::new(None);
    static ref KEYS_STATE: Mutex<Vec<bool>> = Mutex::new(vec![false; 0x100]);
    static ref DISPLAY_STATS: atomic::AtomicBool = atomic::AtomicBool::new(false);
    static ref CLIPBOARD_TRIG: atomic::AtomicBool = atomic::AtomicBool::new(false);
    static ref WINDOW_RECEIVER: Mutex<Option<Receiver<AreaManager>>> = Mutex::new(None);
    static ref WINDOW_SENDER: Mutex<Option<Sender<AreaManager>>> = Mutex::new(None);
    static ref MSG_SENDER: Mutex<Option<Sender<u64>>> = Mutex::new(None);
    static ref SKIP_CLIPBOARD: Mutex<u32> = Mutex::new(0);
    static ref SYNC_KEY_LOCKS_NEEDED: Mutex<bool> = Mutex::new(false);
}

/// Windows keycodes come from raw usb hid keycodes,
/// then transformed to xkb keycodes
/// xkbprint -color -kc :0 - | ps2pdf - > xkbprint.pdf
const KEY_CTRL: usize = 37;
const KEY_SHIFT: usize = 50;
const KEY_ALT: usize = 64;
const KEY_S: usize = 39;
const KEY_C: usize = 54;

const WM_UPDATE_FRAME: UINT = WM_USER + 1;
const WM_WTSSESSION_CHANGE: DWORD = 0x2B1;
//const WTS_SESSION_LOCK: DWORD = 0x7;
const WTS_SESSION_UNLOCK: DWORD = 0x8;
const MIN_CURSOR_SIZE: u32 = 32;
const MAX_CURSOR_SIZE: u32 = 2048;

const MIN_HOTSPOT_SIZE: i32 = -2048;
const MAX_HOTSPOT_SIZE: i32 = 2048;

const SANZU_NAME: &str = "Sanzu";
const SANZU_CLASS: &str = "Sanzu-class";

enum AreaManager {
    CreateArea(usize, String),
    DeleteArea(usize),
}

fn key_state_to_bool(state: lock_keys::LockKeyState) -> bool {
    match state {
        lock_keys::LockKeyState::Enabled => true,
        lock_keys::LockKeyState::Disabled => false,
    }
}

pub fn set_region_clipping(hwnd: HWND, zones: &[Area]) -> Result<()> {
    let rect_bound = RECT {
        left: 10000,
        top: 10000,
        right: 0,
        bottom: 0,
    };

    /* Create master region */
    let rdh = RGNDATAHEADER {
        dwSize: std::mem::size_of::<RGNDATAHEADER>() as u32,
        iType: RDH_RECTANGLES,
        nCount: 0,
        nRgnSize: 0,
        rcBound: rect_bound,
    };
    let rgn = RGNDATA { rdh, Buffer: [0] };

    let total_size = std::mem::size_of::<RGNDATAHEADER>() + 1;
    let hwnd_rgn = unsafe { ExtCreateRegion(null_mut(), total_size as u32, &rgn) };
    if hwnd_rgn.is_null() {
        return Err(anyhow!("Cannot create Region"));
    }
    // XXX TODO: get windows window border thickness or set it to 0
    let border_size = 3;
    trace!("areas:");
    for area in zones {
        trace!("region {:?}", area);
        if !area.mapped {
            continue;
        }
        let rect_bound = RECT {
            left: 10000,
            top: 10000,
            right: 0,
            bottom: 0,
        };
        let len = 1;

        let rect = RECT {
            left: area.position.0 as i32 + border_size,
            top: area.position.1 as i32 + border_size,
            right: area.position.0 as i32 + area.size.0 as i32 + border_size,
            bottom: area.position.1 as i32 + area.size.1 as i32 + border_size,
        };

        let mut rects = vec![rect];
        let rdh = RGNDATAHEADER {
            dwSize: std::mem::size_of::<RGNDATAHEADER>() as u32,
            iType: RDH_RECTANGLES,
            nCount: len as u32,
            nRgnSize: 0,
            rcBound: rect_bound,
        };

        let total_size = std::mem::size_of::<RGNDATAHEADER>() + std::mem::size_of::<RECT>() * len;
        let mut data = vec![0u8; total_size];
        let data_ptr = data.as_mut_ptr();

        let mut rgn = RGNDATA { rdh, Buffer: [0] };

        /* Copy header*/
        let ptr_src_raw = &mut rgn as *mut RGNDATA as *mut _;
        unsafe {
            std::ptr::copy(ptr_src_raw, data_ptr, std::mem::size_of::<RGNDATAHEADER>());
        }

        /* Copy rects */
        let ptr_src_raw = rects.as_mut_ptr() as *mut RECT as *mut _;
        unsafe {
            std::ptr::copy(
                ptr_src_raw,
                data_ptr.add(std::mem::size_of::<RGNDATAHEADER>()),
                std::mem::size_of::<RECT>() * len,
            );
        }

        let ptr_src_raw = data_ptr as *mut RGNDATA as *mut _;

        let new_rgn = unsafe { ExtCreateRegion(null_mut(), total_size as u32, ptr_src_raw) };

        if new_rgn.is_null() {
            return Err(anyhow!("Cannot create Region"));
        }
        // Use drop to keep lifetime of original object through unsafe call
        drop(data);
        drop(rects);

        unsafe { CombineRgn(hwnd_rgn, hwnd_rgn, new_rgn, RGN_OR) };
        let ptr_rgn = new_rgn as *mut c_void as *mut _;
        unsafe {
            DeleteObject(ptr_rgn);
        }
    }

    let ret = unsafe { SetWindowRgn(hwnd as HWND, hwnd_rgn, 0) };
    if ret == 0 {
        return Err(anyhow!("Cannot  SetWindowRgn"));
    }

    Ok(())
}

pub struct ClientWindows {
    pub frame_sender: SyncSender<(Vec<u8>, u32, u32)>,
    pub cursor_sender: Sender<(Vec<u8>, u32, u32, i32, i32)>,
    pub event_receiver: Receiver<tunnel::MessageClient>,
    pub shape_sender: Sender<Vec<Area>>,
    pub cur_areas: Vec<Area>,
    pub width: u16,
    pub height: u16,
    pub clipboard_config: ClipboardConfig,
    pub clipboard_last_value: Option<String>,
    pub printdir: Option<String>,
    /// Sync caps/num/scroll lock
    pub sync_key_locks: bool,
}

impl ClientWindows {
    pub fn build(
        server_size: Option<(u16, u16)>,
        clipboard_config: ClipboardConfig,
        printdir: Option<String>,
        sync_key_locks: bool,
    ) -> (
        ClientWindows,
        FrameReceiver,
        Sender<tunnel::MessageClient>,
        CursorReceiver,
        Receiver<Vec<Area>>,
        Receiver<()>,
    ) {
        // Frame sender is sync to make backpressure to the serveur if we are slower
        let (frame_sender, frame_receiver) = sync_channel(1);
        let (event_sender, event_receiver) = channel();

        let (cursor_sender, cursor_receiver) = channel();
        let (shape_sender, shape_receiver) = channel();

        let (session_sender, session_receiver) = channel();
        SESSION_SENDER.lock().unwrap().replace(session_sender);

        let (width, height) = match server_size {
            Some((width, height)) => (width, height),
            None => {
                let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
                let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };

                //let (width, height) = (640, 480); ////(Monitor::width(), Monitor::height());
                (width as u16, height as u16)
            }
        };

        *SYNC_KEY_LOCKS_NEEDED.lock().unwrap() = true;

        let video_client = ClientWindows {
            frame_sender,
            cursor_sender,
            event_receiver,
            shape_sender,
            cur_areas: vec![],
            width,
            height,
            clipboard_config,
            clipboard_last_value: None,
            printdir,
            sync_key_locks,
        };

        (
            video_client,
            frame_receiver,
            event_sender,
            cursor_receiver,
            shape_receiver,
            session_receiver,
        )
    }
}

// Wrap Direct3DSurface to be sure we call Release on object deallocation
struct Direct3DSurface {
    inner: *mut IDirect3DSurface9,
}

impl Direct3DSurface {
    fn new(surface: *mut IDirect3DSurface9) -> Self {
        Direct3DSurface { inner: surface }
    }

    fn get_inner(&mut self) -> *mut IDirect3DSurface9 {
        self.inner
    }
}

/**
We check the Release return code to ensure the object is freed with success
This will ensure no memory leak during runtime
**/
impl Drop for Direct3DSurface {
    fn drop(&mut self) {
        if let Some(surface) = unsafe { self.inner.as_ref() } {
            let ret = unsafe { surface.Release() };
            if ret != 0 {
                panic!("Error during surface release");
            }
        }
    }
}

// Wrap IDirect3DDevice9 to be sure we call Release on object deallocation
struct Direct3DDevice {
    inner: *mut IDirect3DDevice9,
}

impl Direct3DDevice {
    fn new(device: *mut IDirect3DDevice9) -> Self {
        Direct3DDevice { inner: device }
    }

    fn get_inner(&mut self) -> *mut IDirect3DDevice9 {
        self.inner
    }
}

/**
We check the Release return code to ensure the object is freed with success
This will ensure no memory leak during runtime
**/
impl Drop for Direct3DDevice {
    fn drop(&mut self) {
        if let Some(device) = unsafe { self.inner.as_ref() } {
            let ret = unsafe { device.Release() };
            if ret != 0 {
                panic!("Error during device release");
            }
        }
    }
}

// Wrap IDirect3D9 to be sure we call Release on object deallocation
struct Direct3D {
    inner: *mut IDirect3D9,
}

impl Direct3D {
    fn new(direct3d: *mut IDirect3D9) -> Self {
        Direct3D { inner: direct3d }
    }

    fn get_inner(&mut self) -> *mut IDirect3D9 {
        self.inner
    }
}

/**
We check the Release return code to ensure the object is freed with success
This will ensure no memory leak during runtime
**/
impl Drop for Direct3D {
    fn drop(&mut self) {
        if let Some(direct3d) = unsafe { self.inner.as_ref() } {
            let ret = unsafe { direct3d.Release() };
            if ret != 0 {
                panic!("Error during direct3d release");
            }
        }
    }
}

/**
IDirect3D9 wrapper structure
The fields' drop order is important here:
The surface must be freed before the device, before the direct3d object
We rely on https://doc.rust-lang.org/reference/destructors.html to ensure drop order
'The fields of a struct are dropped in declaration order.'
**/
#[allow(dead_code)]
struct SanzuDirect3D {
    surface: Direct3DSurface,
    device: Direct3DDevice,
    direct3d: Direct3D,
}

impl SanzuDirect3D {
    fn new(direct3d: Direct3D, device: Direct3DDevice, surface: Direct3DSurface) -> Self {
        SanzuDirect3D {
            direct3d,
            device,
            surface,
        }
    }
}

/// # Safety
///
/// Initialise Direct3D by calling unsafe Windows API
unsafe fn init_d3d9(hwnd: HWND, width: u32, height: u32) -> Result<SanzuDirect3D> {
    let p_direct3d = Direct3DCreate9(D3D_SDK_VERSION);
    if p_direct3d.is_null() {
        return Err(anyhow!("Direct3DCreate9 returned null"));
    }

    let mut direct3d = Direct3D::new(p_direct3d);

    *SCREEN_SIZE.lock().unwrap() = (width, height);

    let mut d3dpp = D3DPRESENT_PARAMETERS {
        BackBufferWidth: 0,
        BackBufferHeight: 0,
        BackBufferFormat: D3DFMT_UNKNOWN,
        BackBufferCount: 0,
        MultiSampleType: 0,
        MultiSampleQuality: 0,
        SwapEffect: D3DSWAPEFFECT_DISCARD,
        hDeviceWindow: hwnd,
        Windowed: TRUE,
        EnableAutoDepthStencil: 0,
        AutoDepthStencilFormat: 0,
        Flags: 0,
        FullScreen_RefreshRateInHz: 0,
        PresentationInterval: 0,
    };

    let direct3d_ref = direct3d.get_inner().as_ref().context("Null direct3d")?;

    let mut p_direct3d_device: *mut IDirect3DDevice9 = null_mut();

    if direct3d_ref.CreateDevice(
        D3DADAPTER_DEFAULT,
        D3DDEVTYPE_HAL,
        hwnd,
        D3DCREATE_SOFTWARE_VERTEXPROCESSING,
        &mut d3dpp as *mut _,
        &mut p_direct3d_device as *mut _,
    ) < 0
    {
        direct3d_ref.Release();
        return Err(anyhow!("Cannot create d3d device"));
    }

    let mut device = Direct3DDevice::new(p_direct3d_device);

    let device_ref = device.get_inner().as_ref().context("Null direct3ddevice")?;

    let mut p_direct3d_surface: *mut IDirect3DSurface9 = null_mut();
    if device_ref.CreateOffscreenPlainSurface(
        width,
        height,
        D3DFMT_X8R8G8B8,
        D3DPOOL_DEFAULT,
        &mut p_direct3d_surface as *mut _,
        null_mut(),
    ) != 0
    {
        return Err(anyhow!("Error in CreateOffscreenPlainSurface"));
    }

    let surface = Direct3DSurface::new(p_direct3d_surface);

    let sanzu_direct3d = SanzuDirect3D::new(direct3d, device, surface);

    Ok(sanzu_direct3d)
}

/// Render a frame to the Direct3D context
/// # Safety
///
/// - p_direct3d_device must not be null
/// - p_direct3d_surface must not be null
unsafe fn render(
    sanzu_direct3d: &mut SanzuDirect3D,
    data: Vec<u8>,
    width: u32,
    height: u32,
) -> Result<()> {
    let mut d3d_rect = D3DLOCKED_RECT::default();
    let device = sanzu_direct3d
        .device
        .get_inner()
        .as_ref()
        .context("Null device")?;

    let surface = sanzu_direct3d
        .surface
        .get_inner()
        .as_ref()
        .context("Null surface")?;

    let ret = surface.LockRect(&mut d3d_rect as *mut _, null_mut(), 0);
    if ret != 0 {
        return Err(anyhow!("Error in lockrect {:x?}", ret));
    }

    let mut p_src = data;
    let mut raw_src = p_src.as_mut_ptr();
    let mut p_dest = d3d_rect.pBits;
    let stride = d3d_rect.Pitch;
    let pixel_w_size = width * 4;

    if p_dest.is_null() {
        // Unlock locked surface
        let ret = surface.UnlockRect();
        if ret != 0 {
            return Err(anyhow!("Error in unlockrect {}", ret));
        }
        return Err(anyhow!("Error: pBits is null"));
    } else {
        for _i in 0..height {
            std::ptr::copy_nonoverlapping(raw_src as *const _, p_dest, pixel_w_size as usize);
            p_dest = p_dest.offset(stride as isize);
            raw_src = raw_src.offset(pixel_w_size as isize);
        }
    }

    // Use drop to keep lifetime of original object through unsafe call
    drop(p_src);

    let ret = surface.UnlockRect();
    if ret != 0 {
        return Err(anyhow!("Error in UnlockRect {:?}", ret));
    }

    if device.Clear(
        0,
        null_mut(),
        D3DCLEAR_TARGET,
        D3DCOLOR_XRGB(0, 0, 0),
        1.0,
        0,
    ) != 0
    {
        return Err(anyhow!("Error in Clear: {:?}", ret));
    }

    if device.BeginScene() != 0 {
        return Err(anyhow!("Error in BeginScene: {:?}", ret));
    }

    let mut p_back_buffer: *mut IDirect3DSurface9 = null_mut();
    if device.GetBackBuffer(0, 0, D3DBACKBUFFER_TYPE_MONO, &mut p_back_buffer as *mut _) != 0 {
        return Err(anyhow!("Error in getbackbuffer: {:?}", ret));
    }

    let mut back_buffer = Direct3DSurface::new(p_back_buffer);

    /* Use rect with img size to avoid stretching */
    let new_rect = RECT {
        left: 0,
        top: 0,
        right: width as i32,
        bottom: height as i32,
    };

    if device.StretchRect(
        sanzu_direct3d.surface.get_inner() as *mut _,
        null_mut(),
        back_buffer.get_inner(),
        &new_rect as *const _,
        D3DTEXF_NONE,
    ) != 0
    {
        return Err(anyhow!("Error in StretchRect: {:?}", ret));
    }

    if device.EndScene() != 0 {
        return Err(anyhow!("Error in EndScene: {:?}", ret));
    }

    if device.Present(null_mut(), null_mut(), null_mut(), null_mut()) != 0 {
        return Err(anyhow!("Error in Present: {:?}", ret));
    }
    Ok(())
}

extern "system" fn custom_wnd_proc_sub(
    hwnd: HWND,
    msg: UINT,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_SETFOCUS => {
            info!("got focus {:?}", hwnd);
        }
        WM_KILLFOCUS => {
            info!("lost focus {:?}", hwnd);
        }
        WM_ACTIVATE => {
            info!("activate {:?}", hwnd);
            if let Some(id) = HANDLE_TO_WIN_ID.lock().unwrap().get(&(hwnd as u64)) {
                let eventwinactivate = tunnel::EventWinActivate { id: *id as u32 };
                let msg_event = tunnel::MessageClient {
                    msg: Some(tunnel::message_client::Msg::Activate(eventwinactivate)),
                };
                EVENT_SENDER
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .send(msg_event)
                    .expect("Error in send EventWinActivate");
            }
            MSG_SENDER
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .send(0)
                .expect("Error in send MsgSender");
        }
        WM_DESTROY => {
            unsafe { DestroyWindow(hwnd) };
        }
        _ => {
            trace!("msg: {:?}, {:?}", hwnd, msg);
        }
    }
    unsafe { DefWindowProcA(hwnd, msg, wparam, lparam) }
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
            // If caps/num/scroll locks have changed during out of focus,
            // force keys state synchro
            *SYNC_KEY_LOCKS_NEEDED.lock().unwrap() = true;
        }
        WM_KILLFOCUS => {
            trace!("lost focus");
            /* On focus out, release each pushed keys */
            for (index, key_state) in KEYS_STATE.lock().unwrap().iter_mut().enumerate() {
                if *key_state {
                    *key_state = false;
                    let eventkey = tunnel::EventKey {
                        keycode: index as u32,
                        updown: false,
                    };
                    let msg_event = tunnel::MessageClient {
                        msg: Some(tunnel::message_client::Msg::Key(eventkey)),
                    };
                    EVENT_SENDER
                        .lock()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .send(msg_event)
                        .expect("Error in send key_state");
                }
            }
        }
        WM_INPUT => {
            let mut dwsize = 0u32;
            unsafe {
                GetRawInputData(
                    lparam as *mut _,
                    RID_INPUT,
                    null_mut(),
                    &mut dwsize as *mut _,
                    std::mem::size_of::<RAWINPUTHEADER>() as u32,
                );
            };

            let mut data = vec![0u8; dwsize as usize];
            let data_ptr = data.as_mut_ptr();

            unsafe {
                let ret = GetRawInputData(
                    lparam as *mut _,
                    RID_INPUT,
                    data_ptr as *mut _,
                    &mut dwsize as *mut _,
                    std::mem::size_of::<RAWINPUTHEADER>() as u32,
                );
                assert!(ret == dwsize as u32);
            };

            let raw_input_ptr: PRAWINPUT = data_ptr as *mut _;
            let raw_input: RAWINPUT = unsafe { *raw_input_ptr };
            let raw_input_hdr = raw_input.header;
            let result = if raw_input_hdr.dwType == RIM_TYPEKEYBOARD {
                let data = unsafe { raw_input.data.keyboard() };
                /*
                info!(
                    "data {:x} {:x} {:x} {:x} {:x} {:x}",
                    data.MakeCode,
                    data.Flags,
                    data.Reserved,
                    data.VKey,
                    data.Message,
                    data.ExtraInformation
                );
                 */

                let key_code = utils_win::hid_code_to_hardware_keycode(
                    data.MakeCode as u32,
                    data.Flags as u32,
                    data.VKey as u32,
                );
                let updown = data.Message & 1 == 0;
                (key_code, updown)
            } else {
                (None, true)
            };
            if let (Some(keycode), updown) = result {
                KEYS_STATE.lock().unwrap()[keycode as usize] = updown;
                let eventkey = tunnel::EventKey {
                    keycode: keycode as u32,
                    updown,
                };

                // If Ctrl alt shift s => Generate toggle server logs
                if keycode == KEY_S as u16 && updown {
                    // Ctrl Shift Alt
                    let keys_state = KEYS_STATE.lock().unwrap();
                    if keys_state[KEY_CTRL] && keys_state[KEY_SHIFT] && keys_state[KEY_ALT] {
                        let display_stats = DISPLAY_STATS.load(atomic::Ordering::Acquire);
                        DISPLAY_STATS.store(!display_stats, atomic::Ordering::Release);
                        info!("Toggle server logs");
                    }
                }

                // If Ctrl alt shift c => Trig clipboard event
                if keycode == KEY_C as u16 && updown {
                    // Ctrl Shift Alt
                    let keys_state = KEYS_STATE.lock().unwrap();
                    if keys_state[KEY_CTRL] && keys_state[KEY_SHIFT] && keys_state[KEY_ALT] {
                        CLIPBOARD_TRIG.store(true, atomic::Ordering::Release);
                    }
                }

                let msg_event = tunnel::MessageClient {
                    msg: Some(tunnel::message_client::Msg::Key(eventkey)),
                };
                EVENT_SENDER
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .send(msg_event)
                    .expect("Error in send key_state");
            }
            drop(data);
        }

        WM_UPDATE_FRAME => {}

        WM_MOUSEMOVE => {
            trace!("Move {:?} {:?} {:?} {:x?}", hwnd, msg, wparam, lparam);
            let x = lparam & 0xFFFF;
            let y = lparam >> 16;
            let eventmove = tunnel::EventMove {
                x: (x) as u32,
                y: (y) as u32,
            };
            let msg_event = tunnel::MessageClient {
                msg: Some(tunnel::message_client::Msg::Move(eventmove)),
            };
            EVENT_SENDER
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .send(msg_event)
                .expect("Error in send EventMove");
        }
        WM_LBUTTONDOWN | WM_MBUTTONDOWN | WM_RBUTTONDOWN => {
            trace!("clickdown {:?} {:x} {:?} {:x?}", hwnd, msg, wparam, lparam);
            let x = lparam & 0xFFFF;
            let y = lparam >> 16;
            if msg & 0x200 != 0 {
                let button = msg & 0xF;
                if let Some(button) = match button {
                    1 => Some(1),
                    4 => Some(3),
                    7 => Some(2),
                    _ => None,
                } {
                    let eventbutton = tunnel::EventButton {
                        x: x as u32,
                        y: y as u32,
                        button: button as u32,
                        updown: true,
                    };
                    let msg_event = tunnel::MessageClient {
                        msg: Some(tunnel::message_client::Msg::Button(eventbutton)),
                    };
                    EVENT_SENDER
                        .lock()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .send(msg_event)
                        .expect("Error in send Eventbutton");
                }
            }
        }
        WM_LBUTTONUP | WM_MBUTTONUP | WM_RBUTTONUP => {
            trace!("clickup {:?} {:x} {:?} {:x?}", hwnd, msg, wparam, lparam);
            let x = lparam & 0xFFFF;
            let y = lparam >> 16;
            if msg & 0x200 != 0 {
                let button = msg & 0xF;
                if let Some(button) = match button {
                    2 => Some(1),
                    5 => Some(3),
                    8 => Some(2),
                    _ => None,
                } {
                    let eventbutton = tunnel::EventButton {
                        x: x as u32,
                        y: y as u32,
                        button: button as u32,
                        updown: false,
                    };
                    let msg_event = tunnel::MessageClient {
                        msg: Some(tunnel::message_client::Msg::Button(eventbutton)),
                    };
                    EVENT_SENDER
                        .lock()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .send(msg_event)
                        .expect("Error in send EventButton");
                }
            }
        }
        WM_MOUSEWHEEL => {
            trace!("wheel {:?} {:x} {:x} {:x?}", hwnd, msg, wparam, lparam);
            let x = lparam & 0xFFFF;
            let y = lparam >> 16;
            let button = wparam as i32;
            let button = if button > 0 { 4 } else { 5 };

            // Down
            let eventbutton = tunnel::EventButton {
                x: x as u32,
                y: y as u32,
                button: button as u32,
                updown: true,
            };
            let msg_event = tunnel::MessageClient {
                msg: Some(tunnel::message_client::Msg::Button(eventbutton)),
            };
            EVENT_SENDER
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .send(msg_event)
                .expect("Error in send EventButton");

            // Up
            let eventbutton = tunnel::EventButton {
                x: x as u32,
                y: y as u32,
                button: button as u32,
                updown: false,
            };
            let msg_event = tunnel::MessageClient {
                msg: Some(tunnel::message_client::Msg::Button(eventbutton)),
            };
            EVENT_SENDER
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .send(msg_event)
                .expect("Error in send EventButton");
        }

        WM_DRAWCLIPBOARD => {
            info!("clipboard draw");
            if let Ok(data) = get_clipboard(formats::Unicode) {
                let mut skip_clipboard_guard = SKIP_CLIPBOARD.lock().unwrap();
                if *skip_clipboard_guard > 0 {
                    *skip_clipboard_guard -= 1;
                    // The clipboard may be set by ourself, skip it
                } else {
                    trace!("Send clipboard {}", data);

                    let eventclipboard = tunnel::EventClipboard { data };
                    let msg_event = tunnel::MessageClient {
                        msg: Some(tunnel::message_client::Msg::Clipboard(eventclipboard)),
                    };
                    EVENT_SENDER
                        .lock()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .send(msg_event)
                        .expect("Error in send EventClipboard");
                }
            }
        }
        WM_CHANGECBCHAIN => {
            info!("clipboard chain");
        }
        WM_WTSSESSION_CHANGE => {
            info!("Session change state {:?} {:?}", lparam, wparam);
            if wparam as u32 == WTS_SESSION_UNLOCK {
                // Force d3d re init
                let (width, height) = *SCREEN_SIZE.lock().unwrap();
                let msg = tunnel::EventDisplay {
                    width: width as u32,
                    height: height as u32,
                };
                let msg_event = tunnel::MessageClient {
                    msg: Some(tunnel::message_client::Msg::Display(msg)),
                };
                EVENT_SENDER
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .send(msg_event)
                    .expect("Error in send EventDisplay");
                SESSION_SENDER
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .send(())
                    .expect("Cannot send session unlock");
            }
        }
        WM_DISPLAYCHANGE | WM_SIZE => {
            let width = lparam & 0xFFFF;
            let height = (lparam >> 16) & 0xFFFF;

            info!("Resolution change {}x{}", width, height);
            let msg = tunnel::EventDisplay {
                width: width as u32,
                height: height as u32,
            };
            let msg_event = tunnel::MessageClient {
                msg: Some(tunnel::message_client::Msg::Display(msg)),
            };
            EVENT_SENDER
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .send(msg_event)
                .expect("Error in send EventDisplay");
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            return 0;
        }
        _ => {
            trace!("msg: {:?}, {:?}", hwnd, msg);
        }
    }
    unsafe { DefWindowProcA(hwnd, msg, wparam, lparam) }
}

extern "system" fn hook_callback(_code: i32, w_param: u64, _l_param: i64) -> i64 {
    let mut pos = POINT::default();
    unsafe {
        GetCursorPos(&mut pos as *mut _);
    };

    /* Check button */
    if w_param & 0x200 != 0 {
        let button = w_param & 0xF;
        if let Some((button, updown)) = match button {
            // If click is on the window, we already have it with window
            // messages. So don't get button down into account.
            // We are out of the window, we need to grab mouse button up to give
            // us a chance to release a grabbed window
            2 => Some((1, false)),
            5 => Some((2, false)),
            8 => Some((3, false)),
            _ => None,
        } {
            let eventbutton = tunnel::EventButton {
                x: pos.x as u32,
                y: pos.y as u32,
                button: button as u32,
                updown,
            };
            let msg_event = tunnel::MessageClient {
                msg: Some(tunnel::message_client::Msg::Button(eventbutton)),
            };
            EVENT_SENDER
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .send(msg_event)
                .expect("Error in send EventButton");
        }
    }

    let eventmove = tunnel::EventMove {
        x: (pos.x) as u32,
        y: (pos.y) as u32,
    };
    let msg_event = tunnel::MessageClient {
        msg: Some(tunnel::message_client::Msg::Move(eventmove)),
    };
    EVENT_SENDER
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .send(msg_event)
        .expect("Error in send EventMove");

    0
}
/// Set the client cursor
fn set_window_cursor(cursor_data: &[u8], width: u32, height: u32, xhot: i32, yhot: i32) {
    debug!(
        "Set cursor {}x{} {}x{} {}",
        width,
        height,
        xhot,
        yhot,
        cursor_data.len()
    );
    if width > MAX_CURSOR_SIZE || height > MAX_CURSOR_SIZE {
        error!("Strange cursor size {}x{}", width, height);
        return;
    }

    if (width * height * 4) as usize != cursor_data.len() {
        return;
    }

    if width < 4 || height < 4 {
        // Skip little cursors
        return;
    }

    if xhot > MAX_HOTSPOT_SIZE
        || xhot > MAX_HOTSPOT_SIZE
        || xhot < MIN_HOTSPOT_SIZE
        || yhot < MIN_HOTSPOT_SIZE
    {
        error!("Strange cursor hot {}x{}", xhot, yhot);
        return;
    }

    let xhot = if xhot < 0 { 0 } else { xhot as u16 };
    let yhot = if yhot < 0 { 0 } else { yhot as u16 };

    trace!(
        "cursor {}x{} {},{} {}",
        width,
        height,
        xhot,
        yhot,
        cursor_data.len()
    );

    let mut cursor_bgra = vec![];
    for values in cursor_data.chunks(4) {
        if let &[r, g, b, a] = values {
            cursor_bgra.push(b);
            cursor_bgra.push(g);
            cursor_bgra.push(r);
            cursor_bgra.push(a);
        }
    }

    // Set minimum width to 32 pixels to avoid windows cursor scale
    let (data, width, height) = if width < MIN_CURSOR_SIZE {
        let mut data = vec![];
        for i in 0..height {
            data.append(
                &mut cursor_bgra[(width * 4 * i) as usize..(width * 4 * (i + 1)) as usize].to_vec(),
            );
            data.append(&mut vec![0u8; ((MIN_CURSOR_SIZE - width) * 4) as usize])
        }
        (data, MIN_CURSOR_SIZE, height)
    } else {
        (cursor_bgra.to_owned(), width, height)
    };

    // Set minimum height to 32 pixels to avoid windows cursor scale
    let (data, width, height) = if height < MIN_CURSOR_SIZE {
        let mut data = data;
        data.append(&mut vec![
            0u8;
            (width * (MIN_CURSOR_SIZE - height) * 4) as usize
        ]);
        (data, width, MIN_CURSOR_SIZE)
    } else {
        (cursor_bgra.to_owned(), width, height)
    };

    let (data, width, height) = match width.cmp(&height) {
        Ordering::Less => {
            let mut data = vec![];
            for i in 0..height {
                data.append(
                    &mut data[(width * 4 * i) as usize..(width * 4 * (i + 1)) as usize].to_vec(),
                );
                data.append(&mut vec![0u8; ((height - width) * 4) as usize])
            }
            (data, height, height)
        }
        Ordering::Greater => {
            let mut data = data;
            data.append(&mut vec![0u8; (width * (width - height) * 4) as usize]);
            (data, width, width)
        }
        Ordering::Equal => (data, width, height),
    };

    let mut data = data.clone();
    unsafe {
        let hdc = GetDC(WINHANDLE.load(atomic::Ordering::Acquire));
        let b_col = CreateBitmap(
            width as i32,
            height as i32,
            1,
            32,
            data.as_mut_ptr() as *mut _,
        );

        let b_mask = CreateCompatibleBitmap(hdc, width as i32, height as i32);

        let mut iconinfo = ICONINFO::default();
        iconinfo.fIcon = 0;
        iconinfo.xHotspot = xhot as u32;
        iconinfo.yHotspot = yhot as u32;
        iconinfo.hbmMask = b_mask;
        iconinfo.hbmColor = b_col;

        let hicon = CreateIconIndirect(&mut iconinfo);

        SetCursor(hicon);
        DeleteObject(b_mask as *mut _);
        DeleteObject(b_col as *mut _);
        DestroyIcon(hicon as *mut _);
        ReleaseDC(WINHANDLE.load(atomic::Ordering::Acquire), hdc);
    }
}

pub fn init_wind3d(
    argumets: &ArgumentsClient,
    mut seamless: bool,
    server_size: Option<(u16, u16)>,
) -> Result<Box<dyn Client>> {
    let (
        client_info,
        frame_receiver,
        event_sender,
        cursor_receiver,
        shape_receiver,
        session_receiver,
    ) = ClientWindows::build(
        server_size,
        argumets.clipboard_config,
        argumets.printdir.clone(),
        argumets.sync_key_locks,
    );
    let (screen_width, screen_height) = (client_info.width, client_info.height);
    let window_mode = argumets.window_mode;

    if window_mode {
        seamless = false;
    }
    EVENT_SENDER.lock().unwrap().replace(event_sender);

    let mut x = RAWINPUTDEVICE {
        usUsagePage: 1,
        usUsage: 6,
        dwFlags: RIDEV_NOLEGACY,
        hwndTarget: null_mut(),
    };

    unsafe {
        if RegisterRawInputDevices(
            &mut x as *mut _,
            1,
            std::mem::size_of::<RAWINPUTDEVICE>() as u32,
        ) == 0
        {
            return Err(anyhow!("Error in RegisterRawInputDevices"));
        }
    };

    let (window_sender, window_receiver) = channel();
    WINDOW_SENDER.lock().unwrap().replace(window_sender);

    let (msg_sender, msg_receiver) = channel();

    MSG_SENDER.lock().unwrap().replace(msg_sender);

    thread::spawn(move || {
        let instance_handle = unsafe { GetModuleHandleA(null_mut()) };
        info!("Create window {} {}", screen_width, screen_height);
        let class_name = CString::new(SANZU_CLASS).expect("Error in create CString class name");
        let class_name_ptr = class_name.as_ptr();
        let wc = WNDCLASSEXA {
            cbSize: std::mem::size_of::<WNDCLASSEXA>() as u32,
            hbrBackground: null_mut(),
            lpfnWndProc: Some(custom_wnd_proc),
            lpszClassName: class_name_ptr,
            hInstance: instance_handle,
            ..Default::default()
        };

        if unsafe { RegisterClassExA(&wc) } == 0 {
            panic!("Cannot register class");
        }

        let img_name = CString::new("test.ico").expect("Error in create img path");
        let img_name_ptr = img_name.as_ptr();
        info!("load ico {:?}", img_name);
        let img = unsafe {
            LoadImageA(
                instance_handle,
                img_name_ptr,
                IMAGE_ICON,
                0,
                0,
                LR_DEFAULTSIZE | LR_LOADFROMFILE,
            )
        };
        info!("img {:?}", img);

        let window_name = CString::new(SANZU_NAME).expect("Error in create CString name");
        let window_name_ptr = window_name.as_ptr();

        let mut window_style = WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN;

        if !window_mode {
            window_style |= WS_MAXIMIZE | WS_POPUP | WS_DLGFRAME;
        } else {
            window_style |= WS_OVERLAPPEDWINDOW;
        }

        let window: HWND = unsafe {
            CreateWindowExA(
                0,
                wc.lpszClassName,
                window_name_ptr,
                window_style,
                0,
                0,
                screen_width as i32,
                screen_height as i32,
                null_mut(),
                null_mut(),
                instance_handle,
                null_mut(),
            )
        };
        WINHANDLE.store(window, atomic::Ordering::Release);
        /* Register session events */
        let dll_name = CString::new("wtsapi32.dll").expect("Error in create CString wtsapi32");
        let func_name = CString::new("WTSRegisterSessionNotification")
            .expect("Error in create CString function");
        let handle = unsafe { LoadLibraryA(dll_name.as_ptr()) };
        let func_addr = unsafe { GetProcAddress(handle, func_name.as_ptr()) };
        if !func_addr.is_null() {
            unsafe {
                let wtsregister_session_notification: extern "C" fn(HWND, DWORD) -> u64 =
                    std::mem::transmute(func_addr);
                wtsregister_session_notification(window, 0);
            }
        }

        unsafe { SetClipboardViewer(window) };
        // Use drop to keep lifetime of original object through unsafe call
        drop(window_name);
        drop(class_name);
        if window.is_null() {
            panic!("Cannot create window");
        }

        info!("Init d3d ok");

        if !window_mode {
            let ptr = hook_callback as *const ();
            let function: unsafe extern "system" fn(
                code: i32,
                wParam: usize,
                lParam: isize,
            ) -> isize = unsafe { std::mem::transmute(ptr) };

            let _hook_id = unsafe { SetWindowsHookExA(WH_MOUSE_LL, Some(function), null_mut(), 0) };
        }
        /* Register class for subwindows */
        let class_name = CString::new("subwindows_class").expect("Error in create CString D3D");
        let class_name_ptr = class_name.as_ptr();
        let wc = WNDCLASSEXA {
            cbSize: std::mem::size_of::<WNDCLASSEXA>() as u32,
            hbrBackground: null_mut(),
            lpfnWndProc: Some(custom_wnd_proc_sub),
            lpszClassName: class_name_ptr,
            hInstance: instance_handle,
            ..Default::default()
        };

        if unsafe { RegisterClassExA(&wc) } == 0 {
            panic!("Cannot register class");
        }

        // Render thread
        // Only take one image from the queue. As it's a sync channel, this
        // will add a backpressure to the main thread.
        thread::spawn(move || {
            let mut sanzu_direct3d = None;
            loop {
                if let Ok((data, width, height)) = frame_receiver.recv() {
                    if (width, height) != *SCREEN_SIZE.lock().unwrap()
                        || session_receiver.try_recv().is_ok()
                    {
                        let window = WINHANDLE.load(atomic::Ordering::Acquire);
                        info!("Init d3d for new resolution {}x{}", width, height);
                        match unsafe { init_d3d9(window, width as u32, height as u32) } {
                            Ok(new_sanzu_direct3d) => {
                                sanzu_direct3d = Some(new_sanzu_direct3d);
                            }
                            Err(err) => {
                                error!("Error in Init_d3d9: {:?}", err);
                                sanzu_direct3d = None;
                            }
                        }
                    }
                    let result = {
                        if let Some(sanzu_direct3d) = sanzu_direct3d.as_mut() {
                            unsafe { render(sanzu_direct3d, data, width, height) }
                        } else {
                            Err(anyhow!("No sanzu obj"))
                        }
                    };

                    if let Err(err) = result {
                        error!("Error during render: {:?}", err);
                        info!("re-Init d3d for resolution {}x{}", width, height);
                        let window = WINHANDLE.load(atomic::Ordering::Acquire);
                        match unsafe { init_d3d9(window, width as u32, height as u32) } {
                            Ok(new_sanzu_direct3d) => {
                                sanzu_direct3d = Some(new_sanzu_direct3d);
                            }
                            Err(err) => {
                                warn!("Error in Init_d3d9: {:?}", err);
                                sanzu_direct3d = None;
                            }
                        }
                    }
                }
            }
        });

        // Clipping area thread
        thread::spawn(move || {
            loop {
                // Area
                // Only keep last areas
                if let Ok(areas) = shape_receiver.recv() {
                    if seamless {
                        trace!("receive shape {:?}", areas);
                        if let Err(err) =
                            set_region_clipping(WINHANDLE.load(atomic::Ordering::Acquire), &areas)
                        {
                            error!("Error in set_region_clipping: {:?}", err);
                        }
                    }
                }
            }
        });

        let mut msg = MSG::default();
        let mut stopped = false;
        while msg.message != WM_QUIT && !stopped {
            // Set focus if we activate a sub window
            if msg_receiver.try_recv().is_ok() {
                unsafe {
                    SetFocus(WINHANDLE.load(atomic::Ordering::Acquire));
                };
            }

            // Receive cursor shape
            if let Ok((cursor_data, width, height, xhot, yhot)) = cursor_receiver.try_recv() {
                set_window_cursor(&cursor_data, width, height, xhot, yhot);
            }
            if let Ok(area_mngr) = window_receiver.try_recv() {
                if !seamless {
                    continue;
                }
                match area_mngr {
                    AreaManager::CreateArea(id, name) => {
                        let instance_handle = unsafe { GetModuleHandleA(null_mut()) };

                        let window_name = CString::new(format!("{}: {}", id, name))
                            .expect("Error in create CString D3D");
                        let window_name_ptr = window_name.as_ptr();
                        let window: HWND = unsafe {
                            CreateWindowExA(
                                0,
                                class_name_ptr,
                                window_name_ptr,
                                WS_VISIBLE
                                    | WS_POPUP
                                    | WS_DLGFRAME
                                    | WS_CLIPSIBLINGS
                                    | WS_CLIPCHILDREN,
                                0,
                                0,
                                0i32,
                                0i32,
                                null_mut(),
                                null_mut(),
                                instance_handle,
                                null_mut(),
                            )
                        };
                        info!("New Window {:?}", window);
                        WIN_ID_TO_HANDLE.lock().unwrap().insert(id, window as u64);
                        HANDLE_TO_WIN_ID.lock().unwrap().insert(window as u64, id);
                        unsafe {
                            SendMessageA(
                                (window) as *mut _,
                                WM_SETICON as u32,
                                ICON_BIG as usize,
                                img as isize,
                            )
                        };
                    }
                    AreaManager::DeleteArea(id) => {
                        if let Some(window) = WIN_ID_TO_HANDLE.lock().unwrap().remove(&id) {
                            HANDLE_TO_WIN_ID.lock().unwrap().remove(&window);
                            //unsafe {DestroyWindow((*window) as *mut _)};
                            info!("Del Window {:x}", window);
                            unsafe { SendMessageA((window) as *mut _, WM_CLOSE, 0, 0) };
                        }
                    }
                }
            }

            while unsafe { PeekMessageA(&mut msg as *mut _, null_mut(), 0, 0, PM_REMOVE) } != 0 {
                if msg.message == WM_QUIT {
                    stopped = true;
                    break;
                }
                unsafe { TranslateMessage(&msg) };
                unsafe { DispatchMessageA(&msg) };
            }
            sleep(Duration::from_millis(5));
        }
        info!("Client end");
        unsafe { ExitProcess(0) };
    });
    Ok(Box::new(client_info))
}

impl Client for ClientWindows {
    fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    fn set_cursor(&mut self, cursor_data: &[u8], size: (u32, u32), hot: (u16, u16)) -> Result<()> {
        self.cursor_sender
            .send((
                cursor_data.to_owned(),
                size.0,
                size.1,
                hot.0 as i32,
                hot.1 as i32,
            ))
            .context("Cannot send cursor")
    }

    fn set_img(&mut self, img: &[u8], size: (u32, u32)) -> Result<()> {
        self.frame_sender
            .send((img.to_owned(), size.0, size.1))
            .context("Cannot send frame")
    }

    fn update(&mut self, areas: &HashMap<usize, Area>) -> Result<()> {
        trace!("update {:?}", areas);
        let mut areas_vec: Vec<Area> = areas.values().cloned().collect();
        areas_vec.sort();
        if areas_vec != self.cur_areas {
            debug!("Send new shape");
            // /* Compute additionnal windows */
            let mut areas_added = vec![];
            let cur_ids = self
                .cur_areas
                .iter()
                .map(|area| area.id)
                .collect::<HashSet<usize>>();
            let new_ids = areas_vec
                .iter()
                .map(|area| area.id)
                .collect::<HashSet<usize>>();
            for area in areas_vec.iter() {
                if cur_ids.contains(&area.id) {
                    continue;
                }
                areas_added.push((*area).clone());
            }
            let mut areas_subbed = vec![];
            for area in self.cur_areas.iter() {
                if new_ids.contains(&area.id) {
                    continue;
                }
                areas_subbed.push((*area).clone());
            }

            for area in areas_added.iter() {
                if !area.is_app {
                    continue;
                }
                info!("Win added {:?}", area);
                /* Remove special char for windows title */
                let name = String::from_iter(area.name.chars().map(|c| {
                    if c.is_ascii_graphic() {
                        c
                    } else {
                        ' '
                    }
                }));
                WINDOW_SENDER
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .send(AreaManager::CreateArea(area.id, name))
                    .expect("Cannot send window");
            }
            for area in areas_subbed.iter() {
                if !area.is_app {
                    continue;
                }
                info!("Win subbed {:?}", area);
                WINDOW_SENDER
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .send(AreaManager::DeleteArea(area.id))
                    .expect("Cannot receive window");
            }
            self.shape_sender
                .send(areas_vec.clone())
                .context("Error in send shape")?;
            self.cur_areas = areas_vec;
        }
        Ok(())
    }

    fn set_clipboard(&mut self, data: &str) -> Result<()> {
        *SKIP_CLIPBOARD.lock().unwrap() += 1;
        set_clipboard(formats::Unicode, data)
            .map_err(|err| anyhow!("Err {:?}", err))
            .context("Cannot set clipboard")?;
        Ok(())
    }

    fn poll_events(&mut self) -> Result<tunnel::MessagesClient> {
        let mut events = vec![];
        let mut last_move = None;

        if self.sync_key_locks && *SYNC_KEY_LOCKS_NEEDED.lock().unwrap() {
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

            *SYNC_KEY_LOCKS_NEEDED.lock().unwrap() = false;
        }

        while let Ok(event) = self.event_receiver.try_recv() {
            match event {
                event @ tunnel::MessageClient {
                    msg: Some(tunnel::message_client::Msg::Move { .. }),
                } => last_move = Some(event),
                tunnel::MessageClient {
                    msg:
                        Some(tunnel::message_client::Msg::Clipboard(tunnel::EventClipboard { data })),
                } => {
                    self.clipboard_last_value = Some(data);
                }
                _ => events.push(event),
            }
        }

        if let Some(event) = last_move {
            events.push(event)
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
                if let (true, Some(ref data)) = (
                    CLIPBOARD_TRIG.load(atomic::Ordering::Acquire),
                    &self.clipboard_last_value,
                ) {
                    // If we triggered clipboard send and the clipboard is not empty
                    let eventclipboard = tunnel::EventClipboard {
                        data: data.to_owned(),
                    };
                    let clipboard_msg = tunnel::MessageClient {
                        msg: Some(tunnel::message_client::Msg::Clipboard(eventclipboard)),
                    };
                    events.push(clipboard_msg);
                }
                CLIPBOARD_TRIG.store(false, atomic::Ordering::Release);
            }
        }

        Ok(tunnel::MessagesClient { msgs: events })
    }

    fn display_stats(&self) -> bool {
        DISPLAY_STATS.load(atomic::Ordering::Acquire)
    }

    fn printfile(&self, file: &str) -> Result<()> {
        if let Some(ref printdir) = self.printdir {
            info!("Request to print file {:?}", file);
            if !file.chars().all(|c| {
                (char::is_alphanumeric(c) || char::is_ascii_punctuation(&c))
                    && (c != '/' && 'c' != '\\')
            }) {
                return Err(anyhow!("Bad filename {:?}", file));
            }

            let path = std::path::Path::new(printdir);
            let filepath = path.join(file);
            info!("Print file path {:?}", filepath);
            let filepath_str = filepath.to_str().context("Cannot get path str")?;
            let print_str = CString::new("print").context("Error in create print str")?;
            let print_str_ptr = print_str.as_ptr();
            let filename = CString::new(filepath_str).context("Error in create file path str")?;
            let filename_ptr = filename.as_ptr();
            let ret = unsafe {
                ShellExecuteA(
                    null_mut(),
                    print_str_ptr,
                    filename_ptr,
                    null_mut(),
                    null_mut(),
                    SW_HIDE,
                )
            };
            debug!("ret {:?}", ret);
            if ret as usize > 32 {
                Ok(())
            } else {
                Err(anyhow!("Error during printing {:?}", ret))
            }
        } else {
            Err(anyhow!("Not configured to print"))
        }
    }
}
