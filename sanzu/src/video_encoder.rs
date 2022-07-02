use crate::ffmpeg_helper::{averror, set_option, AVCodec, AVCodecContext, AVFrame, AVPacket};
use crate::yuv_rgb_rs;
use anyhow::{Context, Result};
use ffmpeg::AVPixelFormat;
use ffmpeg_sys_next as ffmpeg;
use std::{
    cmp::Ordering,
    collections::HashMap,
    process,
    ptr::null_mut,
    time::{Duration, Instant},
};

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
cpufeatures::new!(cpuid_ssse3, "ssse3");

/// Hold information to build an encoder
#[derive(Debug)]
pub struct EncoderBuilder {
    context: AVCodecContext,
    codec: AVCodec,
    name: String,
    options: HashMap<String, String>,
    framerate: (i32, i32),
    /// Command to execute to get new options on encoder renewal
    command: Option<String>,
}

fn round_size_up(size: usize) -> usize {
    (size + 0x3F) & !0x3F
}

impl EncoderBuilder {
    /// Creates an encoder
    fn new(name: &str) -> Result<Self> {
        let codec = AVCodec::new_encoder(name).context("Error in new AVCodec")?;
        let context = AVCodecContext::new(&codec).context("Error in new AVCodecContext")?;
        let options = HashMap::new();

        Ok(EncoderBuilder {
            context,
            codec,
            name: name.to_owned(),
            options,
            framerate: (25, 1),
            command: None,
        })
    }

    /// Set codec property
    fn set_option(&mut self, name: &str, val: &str) -> Result<()> {
        debug!("set_option: {} -> {}", name, val);
        self.options.insert(name.to_owned(), val.to_owned());
        unsafe { set_option(self.context.as_mut_ptr() as *mut libc::c_void, name, val) }
    }

    /// Set codec framerate
    fn set_framerate(&mut self, num: i32, den: i32) {
        let framerate = ffmpeg::AVRational { num, den };
        self.framerate = (num, den);
        unsafe {
            (*self.context.as_mut_ptr()).framerate = framerate;
        }
    }

    /// Set command line to exectue to retreive options on encoder regeneration
    fn set_command(&mut self, command: &str) -> Result<()> {
        debug!("set_command: {:?}", command);
        self.command = Some(command.to_owned());
        Ok(())
    }

    /// Generate FFmpeg encoder
    fn open(mut self) -> Result<EncoderFFmpeg> {
        if let Some(ref command) = &self.command {
            info!("Spawn custom command retreive");
            let child = process::Command::new(&command)
                .output()
                .context("Cannot run ffmpeg options command")?;
            let output = std::str::from_utf8(&child.stdout)
                .context("Cannot from utf8 ffmpeg options command")?;
            info!("output: {:?}", output);
            for line in output.lines() {
                let mut split = line.splitn(2, '=');
                let opt_name = split
                    .next()
                    .ok_or_else(|| anyhow!("Cannot split opt name"))?;
                let opt_value = split
                    .next()
                    .ok_or_else(|| anyhow!("Cannot split opt value"))?;
                unsafe {
                    set_option(
                        self.context.as_mut_ptr() as *mut libc::c_void,
                        opt_name,
                        opt_value,
                    )?
                };
            }
        }

        let context_ptr = self.context.as_mut_ptr();
        let codec_ptr = self.codec.as_ptr();
        let mut retval: i32 = unsafe { ffmpeg::avcodec_open2(context_ptr, codec_ptr, null_mut()) };
        if retval < 0 {
            return Err(averror("avcodec_open2", retval));
        }
        let frame = AVFrame::new()?;
        let frame_ptr = frame.as_mut_ptr();
        unsafe {
            (*frame_ptr).format = (*context_ptr).pix_fmt as i32;
            (*frame_ptr).width = (*context_ptr).width;
            (*frame_ptr).height = (*context_ptr).height;
        }

        let width = round_size_up(unsafe { (*context_ptr).width } as usize);
        let height = round_size_up(unsafe { (*context_ptr).height } as usize);

        let image_size_y = width * height;

        retval = unsafe { ffmpeg::av_frame_get_buffer(frame_ptr, 0) };
        if retval < 0 {
            return Err(averror("av_frame_get_buffer", retval));
        }

        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        let has_ssse3 = { cpuid_ssse3::init().get() };
        debug!("Encoder {}x{}", width, height);

        // Image size * 3 / 2 because img may have bigger lane
        Ok(EncoderFFmpeg {
            context: self.context,
            name: self.name.clone(),
            options: self.options.clone(),
            framerate: self.framerate,
            #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
            has_ssse3,
            packet: AVPacket::new()?,
            frame,
            image_y: vec![0; image_size_y],
            image_u: vec![0; image_size_y],
            image_v: vec![0; image_size_y],
            image_uv: vec![0; image_size_y],
            command: self.command.clone(),
            size: (width as u16, height as u16),
        })
    }
}

/// Holds EncoderFFmpeg information
#[derive(Debug)]
pub struct EncoderFFmpeg {
    // Property to re init codec
    name: String,
    options: HashMap<String, String>,
    framerate: (i32, i32),
    // Codec runtime variables
    context: AVCodecContext,
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    has_ssse3: bool,
    packet: AVPacket,
    frame: AVFrame,
    /// Y yuv image part
    image_y: Vec<u8>,
    /// U yuv image part
    image_u: Vec<u8>,
    /// V yuv image part
    image_v: Vec<u8>,
    /// UV yuv image part for nv12
    image_uv: Vec<u8>,
    /// Command to execute to get new options on encoder renewal
    command: Option<String>,
    /// image size
    size: (u16, u16),
}

pub struct EncoderTimings {
    pub times: Vec<(&'static str, Duration)>,
}

pub trait Encoder {
    fn is_raw(&self) -> bool;
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    fn has_ssse3(&self) -> bool;
    fn name(&self) -> String;
    fn options(&self) -> HashMap<String, String>;
    fn framerate(&self) -> (i32, i32);
    fn encode_image(
        &mut self,
        image: &[u8],
        width: u32,
        height: u32,
        bytes_per_line: u32,
        count: i64,
    ) -> Result<(Vec<u8>, EncoderTimings)>;
    fn reload(&self) -> Result<Box<dyn Encoder>>;
    fn change_resolution(&mut self, width: u32, height: u32) -> Result<Box<dyn Encoder>>;
}

impl Encoder for EncoderFFmpeg {
    fn is_raw(&self) -> bool {
        false
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    fn has_ssse3(&self) -> bool {
        self.has_ssse3
    }
    fn name(&self) -> String {
        self.name.clone()
    }
    fn options(&self) -> HashMap<String, String> {
        self.options.clone()
    }
    fn framerate(&self) -> (i32, i32) {
        self.framerate
    }

    fn encode_image(
        &mut self,
        image: &[u8],
        width: u32,
        height: u32,
        bytes_per_line: u32,
        count: i64,
    ) -> Result<(Vec<u8>, EncoderTimings)> {
        let time_start = Instant::now();

        let pixel_format = unsafe { (*self.frame.ptr).format };

        match pixel_format {
            x if x == AVPixelFormat::AV_PIX_FMT_YUV420P as i32 => {
                // yuv420
                let (y_lane, u_lane, v_lane) = unsafe {
                    let y_lane = (*self.frame.ptr).linesize[0] as u32;
                    let u_lane = (*self.frame.ptr).linesize[1] as u32;
                    let v_lane = (*self.frame.ptr).linesize[2] as u32;
                    (y_lane, u_lane, v_lane)
                };

                #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
                {
                    if self.has_ssse3() {
                        yuv_rgb_rs::rgba_to_yuv420_ssse3(
                            width as usize,
                            height as usize,
                            image,
                            bytes_per_line as usize,
                            &mut self.image_y,
                            &mut self.image_u,
                            &mut self.image_v,
                            y_lane as usize,
                            u_lane as usize,
                            v_lane as usize,
                            yuv_rgb_rs::YuvType::ItuT871,
                        );
                    } else {
                        yuv_rgb_rs::rgba_to_yuv420_std(
                            width as usize,
                            height as usize,
                            image,
                            bytes_per_line as usize,
                            &mut self.image_y,
                            &mut self.image_u,
                            &mut self.image_v,
                            y_lane as usize,
                            u_lane as usize,
                            v_lane as usize,
                            yuv_rgb_rs::YuvType::ItuT871,
                        );
                    }
                }
                #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
                {
                    yuv_rgb_rs::rgba_to_yuv420_std_rayon(
                        width as usize,
                        height as usize,
                        image,
                        bytes_per_line as usize,
                        &mut self.image_y,
                        &mut self.image_u,
                        &mut self.image_v,
                        y_lane as usize,
                        u_lane as usize,
                        v_lane as usize,
                        yuv_rgb_rs::YuvType::ItuT871,
                    );
                }

                let y_size = (y_lane * height) as usize;
                let uv_size = (u_lane * height / 2) as usize;

                self.frame
                    .make_writable()
                    .context("Error in make_writable")?;

                self.frame
                    .plane(0, y_size)
                    .copy_from_slice(&self.image_y[0..y_size]);
                self.frame
                    .plane(1, uv_size)
                    .copy_from_slice(&self.image_u[0..uv_size]);
                self.frame
                    .plane(2, uv_size)
                    .copy_from_slice(&self.image_v[0..uv_size]);
            }
            x if x == AVPixelFormat::AV_PIX_FMT_YUV444P as i32 => {
                // yuv444
                let (y_lane, u_lane, v_lane) = unsafe {
                    let y_lane = (*self.frame.ptr).linesize[0] as u32;
                    let u_lane = (*self.frame.ptr).linesize[1] as u32;
                    let v_lane = (*self.frame.ptr).linesize[2] as u32;
                    (y_lane, u_lane, v_lane)
                };

                #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
                {
                    if self.has_ssse3() {
                        yuv_rgb_rs::rgba_to_yuv444_ssse3(
                            width as usize,
                            height as usize,
                            image,
                            bytes_per_line as usize,
                            &mut self.image_y,
                            &mut self.image_u,
                            &mut self.image_v,
                            y_lane as usize,
                            u_lane as usize,
                            v_lane as usize,
                            yuv_rgb_rs::YuvType::ItuT871,
                        );
                    } else {
                        yuv_rgb_rs::rgba_to_yuv444_std(
                            width as usize,
                            height as usize,
                            image,
                            bytes_per_line as usize,
                            &mut self.image_y,
                            &mut self.image_u,
                            &mut self.image_v,
                            y_lane as usize,
                            u_lane as usize,
                            v_lane as usize,
                            yuv_rgb_rs::YuvType::ItuT871,
                        );
                    }
                }
                #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
                {
                    yuv_rgb_rs::rgba_to_yuv444_std_rayon(
                        width as usize,
                        height as usize,
                        image,
                        bytes_per_line as usize,
                        &mut self.image_y,
                        &mut self.image_u,
                        &mut self.image_v,
                        y_lane as usize,
                        u_lane as usize,
                        v_lane as usize,
                        yuv_rgb_rs::YuvType::ItuT871,
                    );
                }

                let y_size = (y_lane * height) as usize;
                let uv_size = (u_lane * height) as usize;

                self.frame
                    .make_writable()
                    .context("Error in make_writable")?;

                self.frame
                    .plane(0, y_size)
                    .copy_from_slice(&self.image_y[0..y_size]);
                self.frame
                    .plane(1, uv_size)
                    .copy_from_slice(&self.image_u[0..uv_size]);
                self.frame
                    .plane(2, uv_size)
                    .copy_from_slice(&self.image_v[0..uv_size]);
            }
            x if x == AVPixelFormat::AV_PIX_FMT_NV12 as i32 => {
                // nv12
                let (y_lane, uv_lane) = unsafe {
                    let y_lane = (*self.frame.ptr).linesize[0] as u32;
                    let uv_lane = (*self.frame.ptr).linesize[1] as u32;
                    (y_lane, uv_lane)
                };

                #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
                {
                    if self.has_ssse3() {
                        yuv_rgb_rs::rgba_to_nv12_ssse3(
                            width as usize,
                            height as usize,
                            image,
                            bytes_per_line as usize,
                            &mut self.image_y,
                            &mut self.image_uv,
                            y_lane as usize,
                            uv_lane as usize,
                            yuv_rgb_rs::YuvType::ItuT871,
                        );
                    } else {
                        yuv_rgb_rs::rgba_to_nv12_std(
                            width as usize,
                            height as usize,
                            image,
                            bytes_per_line as usize,
                            &mut self.image_y,
                            &mut self.image_uv,
                            y_lane as usize,
                            uv_lane as usize,
                            yuv_rgb_rs::YuvType::ItuT871,
                        );
                    }
                }
                #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
                {
                    yuv_rgb_rs::rgba_to_nv12_std(
                        width as usize,
                        height as usize,
                        image,
                        bytes_per_line as usize,
                        &mut self.image_y,
                        &mut self.image_uv,
                        y_lane as usize,
                        uv_lane as usize,
                        yuv_rgb_rs::YuvType::ItuT871,
                    );
                }

                let y_size = (y_lane * height) as usize;
                let uv_size = (uv_lane * height / 2) as usize;

                self.frame
                    .make_writable()
                    .context("Error in make_writable")?;

                self.frame
                    .plane(0, y_size)
                    .copy_from_slice(&self.image_y[0..y_size]);
                self.frame
                    .plane(1, uv_size)
                    .copy_from_slice(&self.image_uv[0..uv_size]);
            }
            x if x == AVPixelFormat::AV_PIX_FMT_RGB0 as i32 => {
                // rgb0
                let image_bpl = bytes_per_line as usize;
                let height = height as usize;

                let plane_bpl = unsafe { (*self.frame.ptr).linesize[0] } as usize;
                let size = plane_bpl * height;
                self.frame
                    .make_writable()
                    .context("Error in make_writable")?;
                let plane = self.frame.plane(0, size);

                match (plane_bpl).cmp(&image_bpl) {
                    Ordering::Equal => {
                        self.frame.plane(0, size).copy_from_slice(&image[0..size]);
                    }
                    Ordering::Less => {
                        for index in 0..height {
                            plane[plane_bpl * index..plane_bpl * (index + 1)].copy_from_slice(
                                &image[image_bpl * index..image_bpl * index + plane_bpl],
                            );
                        }
                    }
                    Ordering::Greater => {
                        for index in 0..height as usize {
                            plane[plane_bpl * index..plane_bpl * index + image_bpl]
                                .copy_from_slice(
                                    &image[image_bpl * index..image_bpl * index + image_bpl],
                                );
                        }
                    }
                }
            }
            _ => {
                return Err(anyhow!("Unsupported pixel format {}", pixel_format));
            }
        };

        unsafe {
            (*self.frame.as_mut_ptr()).pts = count;
        }
        let time_yuv = Instant::now();

        let mut retval = unsafe {
            ffmpeg::avcodec_send_frame(self.context.as_mut_ptr(), self.frame.as_mut_ptr())
        };
        if retval < 0 {
            return Err(averror("avcodec_send_frame", retval));
        }
        let mut buffer = Vec::new();
        while retval >= 0 {
            retval = unsafe {
                ffmpeg::avcodec_receive_packet(self.context.as_mut_ptr(), self.packet.as_mut_ptr())
            };
            if retval == ffmpeg::AVERROR(ffmpeg::EAGAIN) || retval == ffmpeg::AVERROR_EOF {
                break;
            }
            if retval < 0 {
                return Err(anyhow!("Error in avcodec_receive_packet"));
            }
            let slice = unsafe {
                std::slice::from_raw_parts(
                    (*self.packet.as_mut_ptr()).data as *const u8,
                    (*self.packet.as_mut_ptr()).size as usize,
                )
            };
            buffer.extend_from_slice(slice);
            unsafe {
                ffmpeg::av_packet_unref(self.packet.as_mut_ptr());
            }
        }
        let time_encode = Instant::now();
        let duration_yuv = time_yuv - time_start;
        let duration_enc = time_encode - time_yuv;
        let timings = vec![("yuv", duration_yuv), ("enc", duration_enc)];

        Ok((buffer, EncoderTimings { times: timings }))
    }

    fn reload(&self) -> Result<Box<dyn Encoder>> {
        let (width, height) = self.size;
        let options = self.options();
        let framerate = self.framerate();
        let name = self.name();
        let mut builder = EncoderBuilder::new(&name)?;
        for (key, value) in options.iter() {
            builder
                .set_option(key, value)
                .context(format!("Error in set_option {:?} {:?}", key, value))?;
        }
        let video_size = format!("{}x{}", width, height);
        builder
            .set_option("video_size", &video_size)
            .context(format!("Error in set_option video_size {:?}", video_size))?;
        builder.set_framerate(framerate.0, framerate.1);
        if let Some(ref command) = self.command {
            builder.set_command(command)?;
        }

        let encoder = builder.open().context("Error in encoder open")?;
        Ok(Box::new(encoder))
    }

    fn change_resolution(&mut self, width: u32, height: u32) -> Result<Box<dyn Encoder>> {
        let width = width & !1;
        let height = height & !1;
        self.size = (width as u16, height as u16);
        self.reload()
    }
}

/// Dummy video encoder used as passthrough
#[derive(Debug, Default)]
pub struct EncoderNull {}

impl EncoderNull {
    fn new() -> Self {
        EncoderNull::default()
    }
}

impl Encoder for EncoderNull {
    fn is_raw(&self) -> bool {
        true
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    fn has_ssse3(&self) -> bool {
        false
    }
    fn name(&self) -> String {
        "".to_owned()
    }
    fn options(&self) -> HashMap<String, String> {
        HashMap::new()
    }
    fn framerate(&self) -> (i32, i32) {
        (0, 0)
    }
    fn encode_image(
        &mut self,
        image: &[u8],
        _width: u32,
        _height: u32,
        _bytes_per_line: u32,
        _count: i64,
    ) -> Result<(Vec<u8>, EncoderTimings)> {
        Ok((image.to_owned(), EncoderTimings { times: vec![] }))
    }
    fn reload(&self) -> Result<Box<dyn Encoder>> {
        Ok(Box::new(EncoderNull::new()))
    }

    fn change_resolution(&mut self, _width: u32, _height: u32) -> Result<Box<dyn Encoder>> {
        Ok(Box::new(EncoderNull::new()))
    }
}

pub fn init_video_encoder<'a>(
    name: &str,
    global_options: Option<impl Iterator<Item = (&'a String, &'a String)>>,
    codec_options: Option<impl Iterator<Item = (&'a String, &'a String)>>,
    command_options: &Option<String>,
    size: (u16, u16),
) -> Result<Box<dyn Encoder>> {
    // Set log level to FATAL if building release
    #[cfg(not(debug_assertions))]
    unsafe {
        ffmpeg::av_log_set_level(ffmpeg::AV_LOG_FATAL);
    }
    let encoder: Box<dyn Encoder> = match name {
        "null" => Box::new(EncoderNull::new()),
        name => {
            let mut enc = EncoderBuilder::new(name).context("Error in EncoderBuilder")?;

            // Set global options
            if let Some(opts) = global_options {
                for (k, v) in opts {
                    enc.set_option(k, v).context("Error in set option")?;
                }
            }

            // Set codec specific options
            if let Some(opts) = codec_options {
                for (k, v) in opts {
                    enc.set_option(k, v).context("Error set option error")?;
                }
            }

            // Set option command line
            if let Some(ref command) = command_options {
                info!("set ffmpeg options command");
                enc.set_command(command)?;
            }

            let video_size = format!("{}x{}", size.0, size.1);
            unsafe {
                set_option(
                    enc.context.as_mut_ptr() as *mut libc::c_void,
                    "video_size",
                    &video_size,
                )?
            };

            enc.set_framerate(25, 1);
            Box::new(enc.open().context("Error in encoder open")?)
        }
    };
    Ok(encoder)
}

pub fn get_encoder_category(encoder_name: &str) -> Result<String> {
    let codec_name = match encoder_name {
        "libx264" | "h264_nvenc" | "h264_qsv" | "h264_v4l2m2m" => "h264",
        "libx265" | "hevc_nvenc" | "hevc_qsv" => "hevc",
        "null" => "null",
        _ => {
            return Err(anyhow!("Unknown encoder category: {:?}", encoder_name));
        }
    };
    Ok(codec_name.to_owned())
}
