use crate::ffmpeg_helper::{
    averror, set_option, AVCodec, AVCodecContext, AVFrame, AVPacket, AVParser,
};
use crate::yuv_rgb_rs;
use anyhow::{Context, Result};
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
use cpufeatures;
//use ffmpeg::{AVCodecContext, AVCodecParserContext, AVFrame, AVPacket};
use ffmpeg_sys_next as ffmpeg;
use std::{
    collections::HashMap,
    ptr::null_mut,
    time::{Duration, Instant},
};

const AV_NOPTS_VALUE: i64 = -0x8000000000000000;

pub struct DecoderTimings {
    pub times: Vec<(&'static str, Duration)>,
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
cpufeatures::new!(cpuid_ssse3, "ssse3");

/// Initialize a FFmpeg video decoder
pub fn init_video_codec<'a>(
    codec_options: Option<impl Iterator<Item = (&'a String, &'a String)>>,
    name: &str,
) -> Result<Box<dyn Decoder>> {
    let decoder: Box<dyn Decoder> = match name {
        "null" => Box::new(DecoderNull {
            data_rgb: None,
            data_rgba: None,
        }),
        name => {
            let mut decoder = DecoderBuilder::new(name).context("Error in DecoderBuilder")?;
            // Set codec specific options
            if let Some(opts) = codec_options {
                for (k, v) in opts {
                    decoder.set_option(k, v).context("Error set option error")?;
                }
            }
            Box::new(decoder.open().context("Error in decoder open")?)
        }
    };
    Ok(decoder)
}

/// - Retrieve an image from the codec
/// - Convert it from yuv to rgb
/// - Clip it to the requested dimensions and convert it to rgba
///
/// As the video codec may only support size multiple of macroblock size, the
/// retrieved image may be bigger than the requested. We clip this (green border
/// artefact) to fit the graphic client buffer
fn decode(
    decoder: &mut DecoderFFmpeg,
    img_out_width: u16,
    img_out_height: u16,
) -> (Option<()>, Option<DecoderTimings>) {
    let mut img_updated = None;
    let time_start = Instant::now();
    let mut duration_yuv = Duration::new(0, 0);
    let context_ptr = decoder.context.as_mut_ptr();
    let packet_ptr = decoder.packet.as_mut_ptr();
    let frame_ptr = decoder.frame.as_mut_ptr();

    let mut ret = unsafe { ffmpeg::avcodec_send_packet(context_ptr, packet_ptr) };

    if ret < 0 {
        panic!("Error sending a packet for decoding ({:?})", ret);
    }
    let mut duration_decode = Instant::now() - time_start;

    while ret >= 0 {
        let time_start = Instant::now();
        ret = unsafe { ffmpeg::avcodec_receive_frame(context_ptr, frame_ptr) };
        if ret == ffmpeg::AVERROR(ffmpeg::EAGAIN) || ret == ffmpeg::AVERROR_EOF {
            let timings = vec![("dec", duration_decode), ("yuv", duration_yuv)];

            return (img_updated, Some(DecoderTimings { times: timings }));
        }
        if ret < 0 {
            panic!("Error during decoding");
        }

        if img_updated.is_some() {
            warn!("Skip frame");
        }

        duration_decode += Instant::now() - time_start;
        let time_yuv = Instant::now();

        let pixel_format = unsafe { (*frame_ptr).format };

        let img_width = unsafe { (*frame_ptr).linesize[0] as u32 };
        let img_height = unsafe { (*frame_ptr).height as u32 };

        // The decoded image should at least be as big as the requested frame
        if img_width < img_out_width as u32 || img_height < img_out_height as u32 {
            panic!(
                "Invalid image size {}x{} {}x{}",
                img_width, img_height, img_out_width, img_out_height
            );
        }

        // Alloc data_rgba to fit new codec size
        if decoder.data_rgba.is_none() {
            decoder.data_rgba = Some(vec![0u8; img_width as usize * img_height as usize * 4]);
        }
        let data_rgba_ptr = decoder.data_rgba.as_mut().expect("Should not be here");

        // Alloc data_rgb to fit new codec size
        if decoder.data_rgb.is_none() {
            decoder.data_rgb = Some(vec![0u8; img_width as usize * img_height as usize * 3]);
        }

        let final_width = img_width.min(img_out_width as u32);
        let final_height = img_height.min(img_out_height as u32);

        match pixel_format {
            0 => {
                // yuv420
                let p1size = unsafe { (*frame_ptr).linesize[0] * (*frame_ptr).height };
                let p2size = unsafe { (*frame_ptr).linesize[1] * (*frame_ptr).height / 2 };

                /* Y part */
                let slice_y = unsafe {
                    std::slice::from_raw_parts_mut((*frame_ptr).data[0] as *mut u8, p1size as _)
                };

                /* U part */
                let slice_u = unsafe {
                    std::slice::from_raw_parts_mut((*frame_ptr).data[1] as *mut u8, p2size as _)
                };

                /* V part */
                let slice_v = unsafe {
                    std::slice::from_raw_parts_mut((*frame_ptr).data[2] as *mut u8, p2size as _)
                };

                let y_lane = unsafe { (*frame_ptr).linesize[0] as u32 };
                let u_lane = unsafe { (*frame_ptr).linesize[1] as u32 };
                let v_lane = unsafe { (*frame_ptr).linesize[2] as u32 };

                #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
                {
                    if decoder.has_ssse3 {
                        yuv_rgb_rs::yuv420_to_rgba_ssse3(
                            final_width as usize,
                            final_height as usize,
                            slice_y,
                            slice_u,
                            slice_v,
                            y_lane as usize,
                            u_lane as usize,
                            v_lane as usize,
                            data_rgba_ptr,
                            img_out_width as usize * 4,
                            yuv_rgb_rs::YuvType::ItuT871,
                        );
                    } else {
                        yuv_rgb_rs::yuv420_to_rgba_std(
                            final_width as usize,
                            final_height as usize,
                            slice_y,
                            slice_u,
                            slice_v,
                            y_lane as usize,
                            u_lane as usize,
                            v_lane as usize,
                            data_rgba_ptr,
                            img_out_width as usize * 4,
                            yuv_rgb_rs::YuvType::ItuT871,
                        );
                    }
                }
                #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
                {
                    yuv_rgb_rs::yuv420_to_rgba_std(
                        final_width as usize,
                        final_height as usize,
                        slice_y,
                        slice_u,
                        slice_v,
                        y_lane as usize,
                        u_lane as usize,
                        v_lane as usize,
                        data_rgba_ptr,
                        img_out_width as usize * 4,
                        yuv_rgb_rs::YuvType::ItuT871,
                    );
                }
            }
            5 => {
                // yuv444
                let p1size = unsafe { (*frame_ptr).linesize[0] * (*frame_ptr).height };
                let p2size = unsafe { (*frame_ptr).linesize[1] * (*frame_ptr).height };

                /* Y part */
                let slice_y = unsafe {
                    std::slice::from_raw_parts_mut((*frame_ptr).data[0] as *mut u8, p1size as _)
                };

                /* U part */
                let slice_u = unsafe {
                    std::slice::from_raw_parts_mut((*frame_ptr).data[1] as *mut u8, p2size as _)
                };

                /* V part */
                let slice_v = unsafe {
                    std::slice::from_raw_parts_mut((*frame_ptr).data[2] as *mut u8, p2size as _)
                };

                let y_lane = unsafe { (*frame_ptr).linesize[0] as u32 };
                let u_lane = unsafe { (*frame_ptr).linesize[1] as u32 };
                let v_lane = unsafe { (*frame_ptr).linesize[2] as u32 };

                #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
                {
                    if decoder.has_ssse3 {
                        yuv_rgb_rs::yuv444_to_rgba_ssse3(
                            final_width as usize,
                            final_height as usize,
                            slice_y,
                            slice_u,
                            slice_v,
                            y_lane as usize,
                            u_lane as usize,
                            v_lane as usize,
                            data_rgba_ptr,
                            img_out_width as usize * 4,
                            yuv_rgb_rs::YuvType::ItuT871,
                        );
                    } else {
                        yuv_rgb_rs::yuv444_to_rgba_std(
                            final_width as usize,
                            final_height as usize,
                            slice_y,
                            slice_u,
                            slice_v,
                            y_lane as usize,
                            u_lane as usize,
                            v_lane as usize,
                            data_rgba_ptr,
                            img_out_width as usize * 4,
                            yuv_rgb_rs::YuvType::ItuT871,
                        );
                    }
                }
                #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
                {
                    yuv_rgb_rs::yuv444_to_rgba_std(
                        final_width as usize,
                        final_height as usize,
                        slice_y,
                        slice_u,
                        slice_v,
                        y_lane as usize,
                        u_lane as usize,
                        v_lane as usize,
                        data_rgba_ptr,
                        img_out_width as usize * 4,
                        yuv_rgb_rs::YuvType::ItuT871,
                    );
                }
            }
            23 => {
                // nv12
                let p1size = unsafe { (*frame_ptr).linesize[0] * (*frame_ptr).height };
                let p2size = unsafe { (*frame_ptr).linesize[1] * (*frame_ptr).height };

                /* Y part */
                let slice_y = unsafe {
                    std::slice::from_raw_parts_mut((*frame_ptr).data[0] as *mut u8, p1size as _)
                };

                /* UV part */
                let slice_uv = unsafe {
                    std::slice::from_raw_parts_mut((*frame_ptr).data[1] as *mut u8, p2size as _)
                };

                let y_lane = unsafe { (*frame_ptr).linesize[0] as u32 };
                let uv_lane = unsafe { (*frame_ptr).linesize[1] as u32 };

                #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
                {
                    if decoder.has_ssse3 {
                        yuv_rgb_rs::nv12_rgba_ssse3(
                            final_width as usize,
                            final_height as usize,
                            slice_y,
                            slice_uv,
                            y_lane as usize,
                            uv_lane as usize,
                            data_rgba_ptr,
                            (img_out_width as usize * 4) as usize,
                            yuv_rgb_rs::YuvType::ItuT871,
                        );
                    } else {
                        yuv_rgb_rs::nv12_rgba_std(
                            final_width as usize,
                            final_height as usize,
                            slice_y,
                            slice_uv,
                            y_lane as usize,
                            uv_lane as usize,
                            data_rgba_ptr,
                            (img_out_width as usize * 4) as usize,
                            yuv_rgb_rs::YuvType::ItuT871,
                        );
                    }
                }
                #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
                {
                    yuv_rgb_rs::nv12_rgba_std(
                        final_width as usize,
                        final_height as usize,
                        slice_y,
                        slice_uv,
                        y_lane as usize,
                        uv_lane as usize,
                        data_rgba_ptr,
                        (img_out_width as usize * 4) as usize,
                        yuv_rgb_rs::YuvType::ItuT871,
                    );
                }
            }
            _ => {
                panic!("Unsupported pixel format {}", pixel_format);
            }
        }

        duration_yuv += Instant::now() - time_yuv;

        img_updated = Some(());
    }
    let timings = vec![("dec", duration_decode), ("yuv", duration_yuv)];

    (img_updated, Some(DecoderTimings { times: timings }))
}

/// Hold information to build a decoder
#[derive(Debug)]
pub struct DecoderBuilder {
    context: AVCodecContext,
    codec: AVCodec,
    packet: AVPacket,
    parser: AVParser,
    name: String,
    options: HashMap<String, String>,
}

/// Holds DecoderFFmpeg information
#[derive(Debug)]
pub struct DecoderFFmpeg {
    name: String,
    options: HashMap<String, String>,
    context: AVCodecContext,
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    has_ssse3: bool,
    packet: AVPacket,
    frame: AVFrame,
    parser: AVParser,
    /// data_rgba & data_rgb are alloced once to avoid malloc / free / memset
    data_rgba: Option<Vec<u8>>,
    data_rgb: Option<Vec<u8>>,
}

impl DecoderBuilder {
    /// Creates a Decoder
    fn new(name: &str) -> Result<Self> {
        let codec = AVCodec::new_decoder(name).context("Error in find AVCodec")?;
        let context = AVCodecContext::new(&codec).context("Error in new AVCodecContext")?;
        let packet = AVPacket::new().context("Error in new AVPacket")?;
        let parser = AVParser::new(codec.id()).context("Error in new AVParser")?;
        let options = HashMap::new();

        Ok(DecoderBuilder {
            context,
            codec,
            packet,
            parser,
            name: name.to_owned(),
            options,
        })
    }

    /// Set codec property
    fn set_option(&mut self, name: &str, val: &str) -> Result<()> {
        debug!("set_option: {} -> {}", name, val);
        self.options.insert(name.to_owned(), val.to_owned());
        unsafe { set_option(self.context.as_mut_ptr() as *mut libc::c_void, name, val) }
    }

    /// Generate FFmpeg decoder
    fn open(mut self) -> Result<DecoderFFmpeg> {
        let context_ptr = self.context.as_mut_ptr();
        let codec_ptr = self.codec.as_ptr();

        /* open codec */
        let retval = unsafe { ffmpeg::avcodec_open2(context_ptr, codec_ptr, null_mut()) };
        if retval < 0 {
            return Err(averror("avcodec_open2", retval));
        }
        let frame = AVFrame::new()?;
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        let has_ssse3 = { cpuid_ssse3::init().get() };

        Ok(DecoderFFmpeg {
            context: self.context,
            name: self.name.clone(),
            options: self.options.clone(),
            #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
            has_ssse3,
            packet: self.packet,
            parser: self.parser,
            frame,
            data_rgba: None,
            data_rgb: None,
        })
    }
}

pub trait Decoder {
    fn is_raw(&self) -> bool;
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    fn has_ssse3(&self) -> bool;
    fn name(&self) -> String;
    fn data_rgba(&mut self) -> &mut Option<Vec<u8>>;
    fn options(&self) -> HashMap<String, String>;
    fn decode_img(
        &mut self,
        data_in: &[u8],
        img_out_width: u16,
        img_out_height: u16,
        img_bytes_per_line: Option<u16>,
    ) -> (Option<()>, Option<DecoderTimings>);
    fn reload(&self) -> Result<Box<dyn Decoder>>;
}

impl Decoder for DecoderFFmpeg {
    fn is_raw(&self) -> bool {
        false
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    fn has_ssse3(&self) -> bool {
        self.has_ssse3
    }
    fn name(&self) -> String {
        self.name.to_string()
    }
    fn data_rgba(&mut self) -> &mut Option<Vec<u8>> {
        &mut self.data_rgba
    }
    fn options(&self) -> HashMap<String, String> {
        self.options.clone()
    }
    fn decode_img(
        &mut self,
        mut data_in: &[u8],
        img_out_width: u16,
        img_out_height: u16,
        _img_bytes_per_line: Option<u16>,
    ) -> (Option<()>, Option<DecoderTimings>) {
        let mut img_updated = None;
        let mut decode_timings = None;
        let mut duration_parse = Duration::new(0, 0);
        let packet_ptr = self.packet.as_mut_ptr();
        let parser_ptr = self.parser.as_mut_ptr();
        let context_ptr = self.context.as_mut_ptr();

        while !data_in.is_empty() {
            let mut ret = 1;
            let time_start = Instant::now();
            while ret != 0 {
                ret = unsafe {
                    ffmpeg::av_parser_parse2(
                        parser_ptr,
                        context_ptr,
                        &mut (*packet_ptr).data,
                        &mut (*packet_ptr).size,
                        data_in.as_ptr(),
                        data_in.len() as i32,
                        AV_NOPTS_VALUE,
                        AV_NOPTS_VALUE,
                        0,
                    )
                };
                if ret < 0 {
                    panic!("Error while parsing");
                }
                data_in = &data_in[ret as usize..];
            }
            duration_parse += Instant::now() - time_start;

            if img_updated.is_some() {
                warn!("Skip frame");
            }
            unsafe {
                if (*packet_ptr).size != 0 {
                    let result = decode(self, img_out_width, img_out_height);
                    img_updated = result.0;
                    decode_timings = result.1;
                }
            }
        }

        if let Some(ref mut decode_timings) = &mut decode_timings {
            decode_timings.times.push(("parse", duration_parse));
        }
        (img_updated, decode_timings)
    }

    fn reload(&self) -> Result<Box<dyn Decoder>> {
        let options = self.options();
        let name = self.name();
        let mut builder = DecoderBuilder::new(&name)?;
        for (key, value) in options.iter() {
            builder
                .set_option(key, value)
                .context(format!("Error in set_option {:?} {:?}", key, value))?;
        }
        let decoder = builder.open().context("Error in decoder open")?;
        Ok(Box::new(decoder))
    }
}

/// Holds DecoderFFmpeg information
#[derive(Debug)]
pub struct DecoderNull {
    /// data_rgba & data_rgb are alloced once to avoid malloc / free / memset
    data_rgba: Option<Vec<u8>>,
    data_rgb: Option<Vec<u8>>,
}

impl Decoder for DecoderNull {
    fn is_raw(&self) -> bool {
        false
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    fn has_ssse3(&self) -> bool {
        false
    }
    fn name(&self) -> String {
        "null".to_string()
    }
    fn data_rgba(&mut self) -> &mut Option<Vec<u8>> {
        &mut self.data_rgba
    }
    fn options(&self) -> HashMap<String, String> {
        HashMap::new()
    }
    fn decode_img(
        &mut self,
        data_in: &[u8],
        img_out_width: u16,
        img_out_height: u16,
        img_bytes_per_line: Option<u16>,
    ) -> (Option<()>, Option<DecoderTimings>) {
        let img_bytes_per_line = match img_bytes_per_line {
            Some(img_bytes_per_line) => img_bytes_per_line,
            None => {
                return (None, None);
            }
        };
        // Alloc data_rgba to fit new codec size
        if self.data_rgba.is_none() {
            self.data_rgba = Some(vec![
                0u8;
                img_out_width as usize * img_out_height as usize * 4
            ]);
        }
        let data_rgba_ptr = self.data_rgba.as_mut().expect("Should not be here");
        // Alloc data_rgb to fit new codec size
        if self.data_rgb.is_none() {
            self.data_rgb = Some(vec![
                0u8;
                img_out_width as usize * img_out_height as usize * 3
            ]);
        }

        let time_start = Instant::now();
        let bytes_per_line = 4 * img_out_width as usize;
        for row in 0..img_out_height {
            let offset_src = row as usize * img_bytes_per_line as usize;
            let offset_dst = 4 * (row as usize * img_out_width as usize);
            data_rgba_ptr[offset_dst..(offset_dst + bytes_per_line)]
                .clone_from_slice(&data_in[offset_src..(offset_src + bytes_per_line)]);
        }

        let duration_copy = Instant::now() - time_start;
        let decode_timings = DecoderTimings {
            times: vec![("copy", duration_copy)],
        };
        (Some(()), Some(decode_timings))
    }

    fn reload(&self) -> Result<Box<dyn Decoder>> {
        Ok(Box::new(DecoderNull {
            data_rgb: None,
            data_rgba: None,
        }))
    }
}
