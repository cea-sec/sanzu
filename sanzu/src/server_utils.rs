use crate::{
    config::ConfigServer,
    utils::ServerEvent,
    video_encoder::{Encoder, EncoderTimings},
};

use anyhow::Result;

use sanzu_common::tunnel;

pub trait Server {
    fn size(&self) -> (u16, u16);
    /// Copy x11 graphic to the shared memory
    fn grab_frame(&mut self) -> Result<()>;
    /// Set the client clipboard to the desired `data`
    //fn set_clipboard(&mut self, data: &str) -> Result<()>;
    /// Apply client events to the server (mouse move, keys, ...)
    fn handle_client_event(&mut self, msgs: tunnel::MessagesClient) -> Result<Vec<ServerEvent>>;
    /// Retrieve the server x11 events and serialize them using protobuf
    ///
    /// Graphic modifications are monitored using Damage x11 extension
    fn poll_events(&mut self) -> Result<Vec<tunnel::MessageSrv>>;
    /// Encode image
    fn generate_encoded_img(
        &mut self,
        video_encoder: &mut Box<dyn Encoder>,
    ) -> Result<(Vec<tunnel::MessageSrv>, Option<EncoderTimings>)>;
    /// Change server screen resolution
    ///
    /// As we cannot change the current video mode we:
    /// - create a new video mode
    /// - set it to the future resolution
    /// - set the screen to this new mode
    /// - delete the old video mode
    /// If everything is ok, we update the video index state, and recreate a new
    /// frame grabber according to the new resolution
    fn change_resolution(&mut self, config: &ConfigServer, width: u32, height: u32) -> Result<()>;
    fn activate_window(&self, win_id: u32) -> Result<()>;
}
