use anyhow::{Context, Result};
use std::sync::{
    mpsc::{Receiver, Sender},
    Arc, Mutex,
};

use crate::utils::ClipboardSelection;

use x11rb::{
    self,
    connection::Connection,
    protocol::{randr, xproto::ConnectionExt as _, xproto::*},
};

use x11_clipboard::Clipboard;

use encoding_rs::mem::decode_latin1;

/// Convert a xfixes event (for clipboard modification) into a x11 selection event
pub fn convert_event<C: Connection>(conn: &C, window: Window, atom_selection: u32) -> Result<()> {
    let atom_property = conn
        .intern_atom(false, b"XSEL_DATA")
        .context("Error in intern_atom XSEL_DATA")?
        .reply()
        .context("Error in XSEL_DATA reply")?;

    let atom_utf8_target = conn
        .intern_atom(false, b"UTF8_STRING")
        .context("Error in intern_atom UTF8_STRING")?
        .reply()
        .context("Error in UTF8_STRING reply")?;

    let atom_property_a = atom_property.atom;

    conn.convert_selection(
        window,
        atom_selection,
        atom_utf8_target.atom,
        atom_property_a,
        0u32,
    )
    .context("Error in convert_selection")?
    .check()
    .context("Error in convert_selection check")?;

    Ok(())
}

/// Returns the content of the xsel_data clipboard
pub fn get_clipboard<C: Connection>(conn: &C, window: Window) -> Result<String> {
    let atom_property = conn
        .intern_atom(false, b"XSEL_DATA")
        .context("Error in intern_atom XSEL_DATA")?
        .reply()
        .context("Error in intern_atom XSEL_DATA reply")?;

    let ret = conn
        .get_property(
            false,
            window,
            atom_property.atom,
            0u32, // AnyPropertyType
            0,
            0xFFFF,
        )
        .context("Error in get_property")?
        .reply()
        .context("Error in get_property check")?;

    let value: String = match std::str::from_utf8(&ret.value) {
        Ok(value) => value.into(),
        Err(_) => decode_latin1(&ret.value).into(),
    };
    trace!("Clipboard: {:?}", value);

    conn.flush().context("Error in x11rb flush")?;
    Ok(value)
}

/// List video mode
pub fn list_video_mode<C: Connection>(conn: &C, window: Window) -> Result<()> {
    let screen_resources = randr::get_screen_resources(conn, window)
        .context("Error in get_screen_resources")?
        .reply()
        .context("Error in get_screen_resources reply")?;

    let mut offset = 0_usize;
    for (index, mode) in screen_resources.modes.iter().enumerate() {
        let name = String::from_utf8_lossy(
            &screen_resources.names[offset..offset + mode.name_len as usize],
        );
        debug!("mode {} name {:?} {:?}", index, name, mode.id);
        offset += mode.name_len as usize;
    }
    Ok(())
}

/// Get video mode named @name_ref
pub fn get_video_mode<C: Connection>(
    conn: &C,
    window: Window,
    name_ref: &str,
) -> Result<Option<u32>> {
    let screen_resources = randr::get_screen_resources(conn, window)
        .context("Error in get_screen_resources")?
        .reply()
        .context("Error in get_screen_resources reply")?;

    let mut offset = 0_usize;
    for mode in screen_resources.modes.iter() {
        let name = String::from_utf8_lossy(
            &screen_resources.names[offset..offset + mode.name_len as usize],
        );
        if name == name_ref {
            return Ok(Some(mode.id));
        }
        offset += mode.name_len as usize;
    }
    Ok(None)
}

/// Delete video mode named @name_ref
pub fn delete_video_mode_by_name<C: Connection>(
    conn: &C,
    window: Window,
    name_ref: &str,
) -> Result<Option<u32>> {
    let screen_resources = randr::get_screen_resources(conn, window)
        .context("Error in get_screen_resources")?
        .reply()
        .context("Error in get_screen_resources reply")?;

    let mut offset = 0_usize;
    let mut current_output = None;
    for output in screen_resources.outputs.iter() {
        let video_output = randr::get_output_info(conn, *output, 0)
            .context("Error in get_output_info")?
            .reply()
            .context("Error in get_output_info reply")?;

        if video_output.crtc != 0 {
            current_output = Some(*output);
            break;
        }
    }

    let current_output = match current_output {
        None => {
            return Err(anyhow!("Cannot find output"));
        }
        Some(output) => output,
    };
    for mode in screen_resources.modes.iter() {
        let name = String::from_utf8_lossy(
            &screen_resources.names[offset..offset + mode.name_len as usize],
        );
        if name == name_ref {
            randr::delete_output_mode(conn, current_output, mode.id)
                .context("Error in delete_output_mode")?;
            randr::destroy_mode(conn, mode.id).context("Error in destroy_mode")?;

            return Ok(Some(mode.id));
        }
        offset += mode.name_len as usize;
    }
    Ok(None)
}
/// Add video mode
/// Size: (width x height)
/// If we cannot add video mode, clearn the state by removing the dummy mode by name
pub fn add_video_mode<C: Connection>(
    conn: &C,
    window: Window,
    width: u16,
    height: u16,
    name: &str,
    id: usize,
) -> Result<u32> {
    let id = id as u32 + 300;
    // Only width / height seems to be used, default other values
    let mode = randr::ModeInfo {
        id: 200,
        width,
        height,
        dot_clock: 100000000,
        hsync_start: 1000,
        hsync_end: 1000,
        htotal: 1000,
        hskew: 0,
        vsync_start: 1000,
        vsync_end: 1000,
        vtotal: 1000,
        name_len: name.len() as u16,
        mode_flags: randr::ModeFlag::HSYNC_NEGATIVE | randr::ModeFlag::VSYNC_NEGATIVE,
    };

    let name_bytes: Vec<u8> = name.as_bytes().to_owned();

    trace!("Create video mode {:?} ({:?}) {:?}", mode, id, name);
    if let Ok(reply) = randr::create_mode(conn, window, mode, &name_bytes)
        .context("Error in create_mode")?
        .reply()
    {
        Ok(reply.mode)
    } else {
        warn!("Cannot create mode {:?} {}x{}", name, width, height);
        if get_video_mode(conn, window, name)?.is_some() {
            delete_video_mode_by_name(conn, window, name)
                .context("Error in delete_video_mode_by_name")?;
        }
        Err(anyhow!("Cannot create video mode"))
    }
}

/// Set video mode with id @mode
pub fn set_video_mode<C: Connection>(conn: &C, window: Window, mode: u32) -> Result<()> {
    let screen_resources = randr::get_screen_resources(conn, window)
        .context("Error in get_screen_resources")?
        .reply()
        .context("Error in get_screen_resources reply")?;

    for output in screen_resources.outputs.iter() {
        let video_output = randr::get_output_info(conn, *output, 0)
            .context("Error in get_output_info")?
            .reply()
            .context("Error in get_output_info reply")?;

        if video_output.crtc != 0 {
            // Add output mode
            randr::add_output_mode(conn, *output, mode).context("Error in add_output_mode")?;
            // set video mode
            randr::set_crtc_config(
                conn,
                video_output.crtc,
                0,
                0,
                0,
                0,
                mode,
                randr::Rotation::ROTATE0,
                &[*output],
            )
            .context("Error in set_crtc_config")?
            .reply()
            .context("Error in set_crtc_config check")?;
        }
    }
    Ok(())
}

pub fn listen_clipboard(
    selection: ClipboardSelection,
    sender: Sender<String>,
    skip_clipboard: Arc<Mutex<u32>>,
) {
    let clipboard = Clipboard::new().unwrap();
    let selection_atom = match selection {
        ClipboardSelection::Clipboard => clipboard.getter.atoms.clipboard,
        ClipboardSelection::Primary => clipboard.getter.atoms.primary,
    };

    loop {
        if let Ok(curr) = clipboard.load_wait(
            selection_atom,
            clipboard.getter.atoms.utf8_string,
            clipboard.getter.atoms.property,
        ) {
            let curr = String::from_utf8_lossy(&curr);
            let curr = curr.trim_matches('\u{0}').trim();

            if curr.is_empty() {
                continue;
            }

            let mut skip_clipboard_guard = skip_clipboard.lock().unwrap();
            if *skip_clipboard_guard > 0 {
                *skip_clipboard_guard -= 1;
                // The clipboard may be set by ourself, skip it
                continue;
            }
            sender.send(curr.to_owned()).expect("Cannot send clipboard");
        }
    }
}

pub fn set_clipboard(clipboard: &Clipboard, selection: i32, value: &str) -> Result<()> {
    let selection_atom = match selection {
        0 /*ClipboardSelection::Clipboard*/ => clipboard.getter.atoms.clipboard,
        1 /*ClipboardSelection::Primary*/ => clipboard.getter.atoms.primary,
        _ => {
            return Err(anyhow!("Unknown clipboard name"));
        }
    };

    clipboard
        .store(
            selection_atom,
            clipboard.getter.atoms.utf8_string,
            value.as_bytes(),
        )
        .context("Error in clipboard strore")?;
    Ok(())
}

pub fn get_clipboard_events(receiver: &Receiver<String>) -> Option<String> {
    /* Pool clipboard events */
    let mut message = None;
    while let Ok(data) = receiver.try_recv() {
        message = Some(data);
    }
    message
}
