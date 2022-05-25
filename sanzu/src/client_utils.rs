use sanzu_common::tunnel;

use anyhow::Result;
use std::collections::HashMap;

/// Holds information on a server side window.
///
/// TODO: for now, we only support rectangle windows.
#[derive(Clone, Debug)]
pub struct Area {
    pub id: usize,
    pub size: (u16, u16),
    pub position: (i16, i16),
    pub mapped: bool,
}

pub trait Client {
    fn size(&self) -> (u16, u16);
    /// Change the client cursor
    ///
    /// * `cursor_data` - A list of u8. A pixel is 4 u8 (rgba) group.
    /// * `hot` - The cursor center position
    ///
    /// TODO: For now, this x11 code transform the rgba into 1 bit cursor (black /
    /// white) and 1 bit (transparent / not transparent) cursor shape.
    fn set_cursor(&mut self, cursor_data: &[u8], size: (u32, u32), hot: (u16, u16)) -> Result<()>;

    /// Set the client image to `img`, with a size of `width`x`height` in 24bpp (rgb)
    /// Inform `client_info` we will need a graphic update
    fn set_img(&mut self, img: &[u8], size: (u32, u32)) -> Result<()>;
    /// Update the client graphic:
    /// - update the local window shape to match remote windows in seamless
    /// - update the x11 image if needed
    fn update(&mut self, areas: &HashMap<usize, Area>) -> Result<()>;

    /// Set the client clipboard to the desired `data`
    fn set_clipboard(&mut self, data: &str) -> Result<()>;

    /// Retrieve the client x11 events and serialize them using protobuf
    ///
    /// Every monitored client event is sent to the server, except the MouseMove
    /// event. Only the last movement is sent. As we may have a "quick" client
    /// mouse, multiple movements may be captured between two client frames. We
    /// don't want to send a list of mouse movement to the server in this case but
    /// only the last one. This may give server side quirks. The drawback is that we
    /// may have "shortcuts" in the remote mouse position (for example, if you are
    /// drawing a circle on a remote Gimp, this may give a polygon)
    fn poll_events(&mut self) -> Result<tunnel::MessagesClient>;

    fn display_stats(&self) -> bool;

    /// Callback to print file
    fn printfile(&self, file: &str) -> Result<()>;
}
