use rayon::prelude::*;
/// Sourced from from https://github.com/descampsa/yuv2rgb
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
use std::arch::x86_64::{
    __m128i, _mm_add_epi16, _mm_add_epi8, _mm_and_si128, _mm_loadu_si128, _mm_mullo_epi16,
    _mm_packus_epi16, _mm_set1_epi16, _mm_set1_epi8, _mm_setzero_si128, _mm_slli_si128,
    _mm_srai_epi16, _mm_srli_epi16, _mm_storeu_si128, _mm_sub_epi16, _mm_sub_epi8,
    _mm_unpackhi_epi16, _mm_unpackhi_epi8, _mm_unpacklo_epi16, _mm_unpacklo_epi8,
};

fn f32_to_fixed_point(value: f32, precision: u32) -> u8 {
    ((value * ((1 << precision) as f32)) + 0.5) as u8
}
#[derive(Debug)]
pub struct RgbToYuvParam {
    r_factor: u8,  // [Rf]
    g_factor: u8,  // [Rg]
    b_factor: u8,  // [Rb]
    cb_factor: u8, // [CbRange/(255*CbNorm)]
    cr_factor: u8, // [CrRange/(255*CrNorm)]
    y_factor: u8,  // [(YMax-YMin)/255]
    y_offset: u8,  // YMin
}

#[derive(Debug)]
pub struct YuvToRgbParam {
    cb_factor: u8,   // [(255*CbNorm)/CbRange]
    cr_factor: u8,   // [(255*CrNorm)/CrRange]
    g_cb_factor: u8, // [Bf/Gf*(255*CbNorm)/CbRange]
    g_cr_factor: u8, // [Rf/Gf*(255*CrNorm)/CrRange]
    y_factor: u8,    // [(YMax-YMin)/255]
    y_offset: u8,    // YMin
}

fn gen_rgb_to_yuv_param(rf: f32, bf: f32, ymin: f32, ymax: f32, cbcrrange: f32) -> RgbToYuvParam {
    RgbToYuvParam {
        r_factor: f32_to_fixed_point(rf, 8),
        g_factor: 255 - f32_to_fixed_point(rf, 8) - f32_to_fixed_point(bf, 8) + 1,
        b_factor: f32_to_fixed_point(bf, 8),
        cb_factor: f32_to_fixed_point((cbcrrange / 255.0) / (2.0 * (1.0 - bf)), 8),
        cr_factor: f32_to_fixed_point((cbcrrange / 255.0) / (2.0 * (1.0 - rf)), 8),
        y_factor: f32_to_fixed_point((ymax - ymin) / 255.0, 7),
        y_offset: ymin as u8,
    }
}

fn gen_yuv_to_rgb_param(rf: f32, bf: f32, ymin: f32, ymax: f32, cbcrrange: f32) -> YuvToRgbParam {
    YuvToRgbParam {
        cb_factor: f32_to_fixed_point(255.0 * (2.0 * (1.0 - bf)) / cbcrrange, 6),
        cr_factor: f32_to_fixed_point(255.0 * (2.0 * (1.0 - rf)) / cbcrrange, 6),
        g_cb_factor: f32_to_fixed_point(
            bf / (1.0 - bf - rf) * 255.0 * (2.0 * (1.0 - bf)) / cbcrrange,
            7,
        ),
        g_cr_factor: f32_to_fixed_point(
            rf / (1.0 - bf - rf) * 255.0 * (2.0 * (1.0 - rf)) / cbcrrange,
            7,
        ),
        y_factor: f32_to_fixed_point(255.0 / (ymax - ymin), 7),
        y_offset: ymin as u8,
    }
}

pub enum YuvType {
    ItuT871,
    ItuR601,
    ItuR709,
}

fn get_rgb_to_yuv_param(param: YuvType) -> RgbToYuvParam {
    match param {
        YuvType::ItuT871 => {
            // ITU-T T.871 (JPEG)
            gen_rgb_to_yuv_param(0.299, 0.114, 0.0, 255.0, 255.0)
        }
        YuvType::ItuR601 => {
            // ITU-R BT.601-7
            gen_rgb_to_yuv_param(0.299, 0.114, 16.0, 235.0, 224.0)
        }
        YuvType::ItuR709 => {
            // ITU-R BT.709-6
            gen_rgb_to_yuv_param(0.2126, 0.0722, 16.0, 235.0, 224.0)
        }
    }
}

fn get_yuv_to_rgb_param(param: YuvType) -> YuvToRgbParam {
    match param {
        YuvType::ItuT871 => {
            // ITU-T T.871 (JPEG
            gen_yuv_to_rgb_param(0.299, 0.114, 0.0, 255.0, 255.0)
        }
        YuvType::ItuR601 => {
            // ITU-R BT.601-7
            gen_yuv_to_rgb_param(0.299, 0.114, 16.0, 235.0, 224.0)
        }
        YuvType::ItuR709 => {
            // ITU-R BT.709-6
            gen_yuv_to_rgb_param(0.2126, 0.0722, 16.0, 235.0, 224.0)
        }
    }
}

fn clamp(value: i16) -> u8 {
    match value {
        value if value < 0 => 0,
        value if value > 255 => 255,
        _ => value as u8,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn rgb24_yuv420_std(
    width: usize,
    height: usize,
    buffer_rgb: &[u8],
    rgb_stride: usize,
    buffer_y: &mut [u8],
    buffer_u: &mut [u8],
    buffer_v: &mut [u8],
    y_stride: usize,
    u_stride: usize,
    v_stride: usize,
    yuv_type: YuvType,
) {
    let param = get_rgb_to_yuv_param(yuv_type);

    for y in (0..height - 1).step_by(2) {
        let mut y_index1 = y * y_stride;
        let mut y_index2 = (y + 1) * y_stride;

        let mut rgb_index1 = y * rgb_stride;
        let mut rgb_index2 = (y + 1) * rgb_stride;

        let mut u_index = (y / 2) * u_stride;
        let mut v_index = (y / 2) * v_stride;

        for _ in (0..width - 1).step_by(2) {
            // compute yuv for the four pixels, u and v values are summed

            let mut y_tmp = (param.r_factor as u16 * buffer_rgb[rgb_index1] as u16
                + param.g_factor as u16 * buffer_rgb[rgb_index1 + 1] as u16
                + param.b_factor as u16 * buffer_rgb[rgb_index1 + 2] as u16)
                >> 8;
            let mut u_tmp: u16 = buffer_rgb[rgb_index1 + 2] as u16 - y_tmp;
            let mut v_tmp: u16 = buffer_rgb[rgb_index1] as u16 - y_tmp;
            buffer_y[y_index1] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            y_tmp = (param.r_factor as u16 * buffer_rgb[rgb_index1 + 3] as u16
                + param.g_factor as u16 * buffer_rgb[rgb_index1 + 4] as u16
                + param.b_factor as u16 * buffer_rgb[rgb_index1 + 5] as u16)
                >> 8;
            u_tmp += buffer_rgb[rgb_index1 + 5] as u16 - y_tmp;
            v_tmp += buffer_rgb[rgb_index1 + 3] as u16 - y_tmp;
            buffer_y[y_index1 + 1] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            y_tmp = (param.r_factor as u16 * buffer_rgb[rgb_index2] as u16
                + param.g_factor as u16 * buffer_rgb[rgb_index2 + 1] as u16
                + param.b_factor as u16 * buffer_rgb[rgb_index2 + 2] as u16)
                >> 8;
            u_tmp += buffer_rgb[rgb_index2 + 2] as u16 - y_tmp;
            v_tmp += buffer_rgb[rgb_index2] as u16 - y_tmp;
            buffer_y[y_index2] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            y_tmp = (param.r_factor as u16 * buffer_rgb[rgb_index2 + 3] as u16
                + param.g_factor as u16 * buffer_rgb[rgb_index2 + 4] as u16
                + param.b_factor as u16 * buffer_rgb[rgb_index2 + 5] as u16)
                >> 8;
            u_tmp += buffer_rgb[rgb_index2 + 5] as u16 - y_tmp;
            v_tmp += buffer_rgb[rgb_index2 + 3] as u16 - y_tmp;
            buffer_y[y_index2 + 1] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            buffer_u[u_index] = ((((u_tmp >> 2) * param.cb_factor as u16) >> 8) + 128) as u8;
            buffer_v[v_index] = ((((v_tmp >> 2) * param.cb_factor as u16) >> 8) + 128) as u8;

            rgb_index1 += 6;
            rgb_index2 += 6;
            y_index1 += 2;
            y_index2 += 2;
            u_index += 1;
            v_index += 1;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn rgba_to_yuv420_std(
    width: usize,
    height: usize,
    buffer_rgba: &[u8],
    rgba_stride: usize,
    buffer_y: &mut [u8],
    buffer_u: &mut [u8],
    buffer_v: &mut [u8],
    y_stride: usize,
    u_stride: usize,
    v_stride: usize,
    yuv_type: YuvType,
) {
    let param = get_rgb_to_yuv_param(yuv_type);
    for y in (0..height - 1).step_by(2) {
        let mut y_index1 = y * y_stride;
        let mut y_index2 = (y + 1) * y_stride;

        let mut rgba_index1 = y * rgba_stride;
        let mut rgba_index2 = (y + 1) * rgba_stride;

        let mut u_index = (y / 2) * u_stride;
        let mut v_index = (y / 2) * v_stride;

        for _ in (0..width - 1).step_by(2) {
            // compute yuv for the four pixels, u and v values are summed
            let mut y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index1] as u16
                + param.g_factor as u16 * buffer_rgba[rgba_index1 + 1] as u16
                + param.b_factor as u16 * buffer_rgba[rgba_index1 + 2] as u16)
                >> 8;
            let mut u_tmp = buffer_rgba[rgba_index1 + 2] as u16 - y_tmp;
            let mut v_tmp = buffer_rgba[rgba_index1] as u16 - y_tmp;
            buffer_y[y_index1] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index1 + 4] as u16
                + param.g_factor as u16 * buffer_rgba[rgba_index1 + 5] as u16
                + param.b_factor as u16 * buffer_rgba[rgba_index1 + 6] as u16)
                >> 8;
            u_tmp += buffer_rgba[rgba_index1 + 6] as u16 - y_tmp;
            v_tmp += buffer_rgba[rgba_index1 + 4] as u16 - y_tmp;
            buffer_y[y_index1 + 1] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index2] as u16
                + param.g_factor as u16 * buffer_rgba[rgba_index2 + 1] as u16
                + param.b_factor as u16 * buffer_rgba[rgba_index2 + 2] as u16)
                >> 8;
            u_tmp += buffer_rgba[rgba_index2 + 2] as u16 - y_tmp;
            v_tmp += buffer_rgba[rgba_index2] as u16 - y_tmp;
            buffer_y[y_index2] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index2 + 4] as u16
                + param.g_factor as u16 * buffer_rgba[rgba_index2 + 5] as u16
                + param.b_factor as u16 * buffer_rgba[rgba_index2 + 6] as u16)
                >> 8;
            u_tmp += buffer_rgba[rgba_index2 + 6] as u16 - y_tmp;
            v_tmp += buffer_rgba[rgba_index2 + 4] as u16 - y_tmp;
            buffer_y[y_index2 + 1] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            buffer_u[u_index] = ((((u_tmp >> 2) * param.cb_factor as u16) >> 8) + 128) as u8;
            buffer_v[v_index] = ((((v_tmp >> 2) * param.cb_factor as u16) >> 8) + 128) as u8;

            rgba_index1 += 8;
            rgba_index2 += 8;
            y_index1 += 2;
            y_index2 += 2;
            u_index += 1;
            v_index += 1;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn rgba_to_yuv444_std(
    width: usize,
    height: usize,
    buffer_rgba: &[u8],
    rgba_stride: usize,
    buffer_y: &mut [u8],
    buffer_u: &mut [u8],
    buffer_v: &mut [u8],
    y_stride: usize,
    u_stride: usize,
    v_stride: usize,
    yuv_type: YuvType,
) {
    let param = get_rgb_to_yuv_param(yuv_type);
    for y in 0..height {
        let mut y_index1 = y * y_stride;

        let mut rgba_index = y * rgba_stride;

        let mut u_index = y * u_stride;
        let mut v_index = y * v_stride;
        for _ in 0..width {
            // compute yuv for the four pixels, u and v values are summed
            let y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index] as u16
                + param.g_factor as u16 * buffer_rgba[rgba_index + 1] as u16
                + param.b_factor as u16 * buffer_rgba[rgba_index + 2] as u16)
                >> 8;
            let u_tmp = buffer_rgba[rgba_index + 2] as u16 - y_tmp;
            let v_tmp = buffer_rgba[rgba_index] as u16 - y_tmp;
            buffer_y[y_index1] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            buffer_u[u_index] = (((u_tmp * param.cb_factor as u16) >> 8) + 128) as u8;
            buffer_v[v_index] = (((v_tmp * param.cb_factor as u16) >> 8) + 128) as u8;

            rgba_index += 4;
            y_index1 += 1;
            u_index += 1;
            v_index += 1;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn yuv420_to_rgba_std(
    width: usize,
    height: usize,
    buffer_y: &[u8],
    buffer_u: &[u8],
    buffer_v: &[u8],
    y_stride: usize,
    u_stride: usize,
    v_stride: usize,
    buffer_rgba: &mut [u8],
    rgba_stride: usize,
    yuv_type: YuvType,
) {
    let param = get_yuv_to_rgb_param(yuv_type);
    for y in (0..height - 1).step_by(2) {
        let mut rgba_index1 = y * rgba_stride;
        let mut rgba_index2 = (y + 1) * rgba_stride;

        let mut u_index = (y / 2) * u_stride;
        let mut v_index = (y / 2) * v_stride;

        let mut y_index1 = y * y_stride;
        let mut y_index2 = (y + 1) * y_stride;

        for _ in (0..width - 1).step_by(2) {
            let u_tmp = buffer_u[u_index] as i16 - 128;
            let v_tmp = buffer_v[v_index] as i16 - 128;

            let b_cb_offset = (param.cb_factor as i16 * u_tmp) >> 6;
            let r_cr_offset = (param.cr_factor as i16 * v_tmp) >> 6;
            let g_cbcr_offset =
                (param.g_cb_factor as i16 * u_tmp + param.g_cr_factor as i16 * v_tmp) >> 7;

            let y_tmp =
                (param.y_factor as i16 * (buffer_y[y_index1] as i16 - param.y_offset as i16)) >> 7;
            buffer_rgba[rgba_index1] = clamp(y_tmp + r_cr_offset);
            buffer_rgba[rgba_index1 + 1] = clamp(y_tmp - g_cbcr_offset);
            buffer_rgba[rgba_index1 + 2] = clamp(y_tmp + b_cb_offset);

            let y_tmp = (param.y_factor as i16
                * (buffer_y[y_index1 + 1] as i16 - param.y_offset as i16))
                >> 7;
            buffer_rgba[rgba_index1 + 4] = clamp(y_tmp + r_cr_offset);
            buffer_rgba[rgba_index1 + 5] = clamp(y_tmp - g_cbcr_offset);
            buffer_rgba[rgba_index1 + 6] = clamp(y_tmp + b_cb_offset);

            let y_tmp =
                (param.y_factor as i16 * (buffer_y[y_index2] as i16 - param.y_offset as i16)) >> 7;
            buffer_rgba[rgba_index2] = clamp(y_tmp + r_cr_offset);
            buffer_rgba[rgba_index2 + 1] = clamp(y_tmp - g_cbcr_offset);
            buffer_rgba[rgba_index2 + 2] = clamp(y_tmp + b_cb_offset);

            let y_tmp = (param.y_factor as i16
                * (buffer_y[y_index2 + 1] as i16 - param.y_offset as i16))
                >> 7;
            buffer_rgba[rgba_index2 + 4] = clamp(y_tmp + r_cr_offset);
            buffer_rgba[rgba_index2 + 5] = clamp(y_tmp - g_cbcr_offset);
            buffer_rgba[rgba_index2 + 6] = clamp(y_tmp + b_cb_offset);

            rgba_index1 += 8;
            rgba_index2 += 8;
            y_index1 += 2;
            y_index2 += 2;
            u_index += 1;
            v_index += 1;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn yuv444_to_rgba_std(
    width: usize,
    height: usize,
    buffer_y: &[u8],
    buffer_u: &[u8],
    buffer_v: &[u8],
    y_stride: usize,
    u_stride: usize,
    v_stride: usize,
    buffer_rgba: &mut [u8],
    rgba_stride: usize,
    yuv_type: YuvType,
) {
    let param = get_yuv_to_rgb_param(yuv_type);
    for y in 0..height {
        let mut rgba_index = y * rgba_stride;
        let mut y_index = y * y_stride;

        let mut u_index = y * u_stride;
        let mut v_index = y * v_stride;

        for _ in 0..width {
            let u_tmp = buffer_u[u_index] as i16 - 128;
            let v_tmp = buffer_v[v_index] as i16 - 128;

            let b_cb_offset = (param.cb_factor as i16 * u_tmp) >> 6;
            let r_cr_offset = (param.cr_factor as i16 * v_tmp) >> 6;
            let g_cbcr_offset =
                (param.g_cb_factor as i16 * u_tmp + param.g_cr_factor as i16 * v_tmp) >> 7;

            let y_tmp =
                (param.y_factor as i16 * (buffer_y[y_index] as i16 - param.y_offset as i16)) >> 7;
            buffer_rgba[rgba_index] = clamp(y_tmp + r_cr_offset);
            buffer_rgba[rgba_index + 1] = clamp(y_tmp - g_cbcr_offset);
            buffer_rgba[rgba_index + 2] = clamp(y_tmp + b_cb_offset);

            rgba_index += 4;
            y_index += 1;
            u_index += 1;
            v_index += 1;
        }
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
macro_rules! unpack_rgb32_step {
    (
        $rs1: ident,
        $rs2: ident,
        $rs3: ident,
        $rs4: ident,
        $rs5: ident,
        $rs6: ident,
        $rs7: ident,
        $rs8: ident
    ) => {{
        let rd1 = _mm_unpacklo_epi8($rs1, $rs5);
        let rd2 = _mm_unpackhi_epi8($rs1, $rs5);
        let rd3 = _mm_unpacklo_epi8($rs2, $rs6);
        let rd4 = _mm_unpackhi_epi8($rs2, $rs6);
        let rd5 = _mm_unpacklo_epi8($rs3, $rs7);
        let rd6 = _mm_unpackhi_epi8($rs3, $rs7);
        let rd7 = _mm_unpacklo_epi8($rs4, $rs8);
        let rd8 = _mm_unpackhi_epi8($rs4, $rs8);

        (rd1, rd2, rd3, rd4, rd5, rd6, rd7, rd8)
    }};
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
macro_rules! rgb32_to_r_g_b_a {
    (
        $rgb1: ident,
        $rgb2: ident,
        $rgb3: ident,
        $rgb4: ident,
        $rgb5: ident,
        $rgb6: ident,
        $rgb7: ident,
        $rgb8: ident
    ) => {{
        let (tmp1, tmp2, tmp3, tmp4, tmp5, tmp6, tmp7, tmp8) =
            unpack_rgb32_step!($rgb1, $rgb2, $rgb3, $rgb4, $rgb5, $rgb6, $rgb7, $rgb8);
        let (rgb1, rgb2, rgb3, rgb4, rgb5, rgb6, rgb7, rgb8) =
            unpack_rgb32_step!(tmp1, tmp2, tmp3, tmp4, tmp5, tmp6, tmp7, tmp8);
        let (tmp1, tmp2, tmp3, tmp4, tmp5, tmp6, tmp7, tmp8) =
            unpack_rgb32_step!(rgb1, rgb2, rgb3, rgb4, rgb5, rgb6, rgb7, rgb8);
        let (rgb1, rgb2, rgb3, _rgb4, rgb5, rgb6, rgb7, _rgb8) =
            unpack_rgb32_step!(tmp1, tmp2, tmp3, tmp4, tmp5, tmp6, tmp7, tmp8);
        (rgb1, rgb2, rgb3, rgb4, rgb5, rgb6, rgb7, rgb8)
    }};
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
macro_rules! load_rgba_4_x_2 {
    (
        $buffer_rgba: ident,
        $rgba_index1: expr,
        $rgba_index2: expr
    ) => {{
        let rgba_ptr1_0 =
            &$buffer_rgba[$rgba_index1] as *const u8 as *const std::arch::x86_64::__m128i;
        let rgba_ptr1_1 =
            &$buffer_rgba[$rgba_index1 + 16] as *const u8 as *const std::arch::x86_64::__m128i;
        let rgba_ptr1_2 =
            &$buffer_rgba[$rgba_index1 + 32] as *const u8 as *const std::arch::x86_64::__m128i;
        let rgba_ptr1_3 =
            &$buffer_rgba[$rgba_index1 + 48] as *const u8 as *const std::arch::x86_64::__m128i;

        let rgba_ptr2_0 =
            &$buffer_rgba[$rgba_index2] as *const u8 as *const std::arch::x86_64::__m128i;
        let rgba_ptr2_1 =
            &$buffer_rgba[$rgba_index2 + 16] as *const u8 as *const std::arch::x86_64::__m128i;
        let rgba_ptr2_2 =
            &$buffer_rgba[$rgba_index2 + 32] as *const u8 as *const std::arch::x86_64::__m128i;
        let rgba_ptr2_3 =
            &$buffer_rgba[$rgba_index2 + 48] as *const u8 as *const std::arch::x86_64::__m128i;

        let rgba1 = _mm_loadu_si128(rgba_ptr1_0);
        let rgba2 = _mm_loadu_si128(rgba_ptr1_1);
        let rgba3 = _mm_loadu_si128(rgba_ptr1_2);
        let rgba4 = _mm_loadu_si128(rgba_ptr1_3);
        let rgba5 = _mm_loadu_si128(rgba_ptr2_0);
        let rgba6 = _mm_loadu_si128(rgba_ptr2_1);
        let rgba7 = _mm_loadu_si128(rgba_ptr2_2);
        let rgba8 = _mm_loadu_si128(rgba_ptr2_3);
        (rgba1, rgba2, rgba3, rgba4, rgba5, rgba6, rgba7, rgba8)
    }};
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
macro_rules! r16_g16_b16_to_y16_u16_v16 {
    (
        $param: ident,
        $r_16: ident,
        $g_16: ident,
        $b_16: ident
    ) => {{
        let y1_16 = _mm_add_epi16(
            _mm_mullo_epi16($r_16, _mm_set1_epi16($param.r_factor as i16)),
            _mm_mullo_epi16($g_16, _mm_set1_epi16($param.g_factor as i16)),
        );
        let y1_16 = _mm_add_epi16(
            y1_16,
            _mm_mullo_epi16($b_16, _mm_set1_epi16($param.b_factor as i16)),
        );
        let y1_16 = _mm_srli_epi16(y1_16, 8);
        let cb1_16 = _mm_sub_epi16($b_16, y1_16);
        let cr1_16 = _mm_sub_epi16($r_16, y1_16);
        (y1_16, cb1_16, cr1_16)
    }};
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
macro_rules! r_g_b_lo_to_y16_u16_v16 {
    (
        $param: ident,
        $rgb1: ident,
        $rgb2: ident,
        $rgb3: ident
    ) => {{
        let r_16 = _mm_unpacklo_epi8($rgb1, _mm_setzero_si128());
        let g_16 = _mm_unpacklo_epi8($rgb2, _mm_setzero_si128());
        let b_16 = _mm_unpacklo_epi8($rgb3, _mm_setzero_si128());
        r16_g16_b16_to_y16_u16_v16!($param, r_16, g_16, b_16)
    }};
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
macro_rules! r_g_b_hi_to_y16_u16_v16 {
    (
        $param: ident,
        $rgb1: ident,
        $rgb2: ident,
        $rgb3: ident
    ) => {{
        let r_16 = _mm_unpackhi_epi8($rgb1, _mm_setzero_si128());
        let g_16 = _mm_unpackhi_epi8($rgb2, _mm_setzero_si128());
        let b_16 = _mm_unpackhi_epi8($rgb3, _mm_setzero_si128());

        r16_g16_b16_to_y16_u16_v16!($param, r_16, g_16, b_16)
    }};
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
macro_rules! rescale_y {
    (
        $param: ident,
        $y_16: ident
    ) => {{
        _mm_add_epi16(
            _mm_srli_epi16(
                _mm_mullo_epi16($y_16, _mm_set1_epi16($param.y_factor as i16)),
                7,
            ),
            _mm_set1_epi16($param.y_offset as i16),
        )
    }};
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
macro_rules! rescale_uv {
    (
        $param: ident,
        $cb_16: ident,
        $cr_16: ident
    ) => {{
        let cb_16 = _mm_add_epi16(
            _mm_srai_epi16(
                _mm_mullo_epi16($cb_16, _mm_set1_epi16($param.cb_factor as i16)),
                8,
            ),
            _mm_set1_epi16(128),
        );
        let cr_16 = _mm_add_epi16(
            _mm_srai_epi16(
                _mm_mullo_epi16($cr_16, _mm_set1_epi16($param.cr_factor as i16)),
                8,
            ),
            _mm_set1_epi16(128),
        );

        (cb_16, cr_16)
    }};
}

#[allow(clippy::too_many_arguments)]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn rgba_to_yuv420_step(
    param: &RgbToYuvParam,
    buffer_rgba: &[u8],
    buffer_y: &mut [u8],
    buffer_u: &mut [u8],
    buffer_v: &mut [u8],
    rgba_index1: usize,
    rgba_index2: usize,
    y_index1: usize,
    y_index2: usize,
    u_index: usize,
    v_index: usize,
) {
    let (rgba1, rgba2, rgba3, rgba4, rgba5, rgba6, rgba7, rgba8) =
        load_rgba_4_x_2!(buffer_rgba, rgba_index1, rgba_index2);

    /* first compute Y', (B-Y') and (R-Y'), in 16bits values, for the first line
    Y is saved for each pixel, while only sums of (B-Y') and (R-Y') for pairs of adjacents
    pixels are saved */

    let (col_r_1, col_g_1, col_b_1, _alpha1, col_r_2, col_g_2, col_b_2, _alpha2) =
        rgb32_to_r_g_b_a!(rgba1, rgba2, rgba3, rgba4, rgba5, rgba6, rgba7, rgba8);

    let (y1_16, cb1_16, cr1_16) = r_g_b_lo_to_y16_u16_v16!(param, col_r_1, col_g_1, col_b_1);
    let (y2_16, cb2_16, cr2_16) = r_g_b_lo_to_y16_u16_v16!(param, col_r_2, col_g_2, col_b_2);

    let cb1_16 = _mm_add_epi16(cb1_16, cb2_16);
    let cr1_16 = _mm_add_epi16(cr1_16, cr2_16);

    /* Rescale Y' to Y, pack it to 8bit values and save it */

    let y1_16 = rescale_y!(param, y1_16);
    let y2_16 = rescale_y!(param, y2_16);
    let y_val = _mm_packus_epi16(y1_16, y2_16);
    let y_val = _mm_unpackhi_epi8(_mm_slli_si128(y_val, 8), y_val);

    let y_ptr1_0 = &mut buffer_y[y_index1] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(y_ptr1_0, y_val);

    /* same for the second line, compute Y', (B-Y') and (R-Y'), in 16bits values
    Y is saved for each pixel, while only sums of (B-Y') and (R-Y') for pairs of adjacents
    pixels are added to the previous values*/

    let (y1_16, cb3_16, cr3_16) = r_g_b_hi_to_y16_u16_v16!(param, col_r_1, col_g_1, col_b_1);

    let cb1_16 = _mm_add_epi16(cb1_16, cb3_16);
    let cr1_16 = _mm_add_epi16(cr1_16, cr3_16);

    let (y2_16, cb4_16, cr4_16) = r_g_b_hi_to_y16_u16_v16!(param, col_r_2, col_g_2, col_b_2);

    let cb1_16 = _mm_add_epi16(cb1_16, cb4_16);
    let cr1_16 = _mm_add_epi16(cr1_16, cr4_16);

    /* Rescale Y' to Y, pack it to 8bit values and save it */

    let y1_16 = rescale_y!(param, y1_16);
    let y2_16 = rescale_y!(param, y2_16);

    let y_val = _mm_packus_epi16(y1_16, y2_16);
    let y_val = _mm_unpackhi_epi8(_mm_slli_si128(y_val, 8), y_val);

    let y_ptr2_0 = &mut buffer_y[y_index2] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(y_ptr2_0, y_val);

    /* Rescale Cb and Cr to their final range */
    let cb1_16 = _mm_srai_epi16(cb1_16, 2);
    let cr1_16 = _mm_srai_epi16(cr1_16, 2);

    let (cb1_16, cr1_16) = rescale_uv!(param, cb1_16, cr1_16);

    /* do the same again with next data */

    let (rgba1, rgba2, rgba3, rgba4, rgba5, rgba6, rgba7, rgba8) =
        load_rgba_4_x_2!(buffer_rgba, rgba_index1 + 64, rgba_index2 + 64);

    /* unpack rgb24 data to r, g and b data in separate channels
       see rgb.txt to get an idea of the algorithm, note that we only go to the next to last step
       here, because averaging in horizontal direction is easier like this
       The last step is applied further on the Y channel only
    */

    let (col_r_1, col_g_1, col_b_1, _alpha1, col_r_2, col_g_2, col_b_2, _alpha2) =
        rgb32_to_r_g_b_a!(rgba1, rgba2, rgba3, rgba4, rgba5, rgba6, rgba7, rgba8);

    /* first compute Y', (B-Y') and (R-Y'), in 16bits values, for the first line
      Y is saved for each pixel, while only sums of (B-Y') and (R-Y') for pairs of adjacents
      pixels are saved
    */
    let (y1_16, cb2_16, cr2_16) = r_g_b_lo_to_y16_u16_v16!(param, col_r_1, col_g_1, col_b_1);
    let (y2_16, cb3_16, cr3_16) = r_g_b_lo_to_y16_u16_v16!(param, col_r_2, col_g_2, col_b_2);

    let cb2_16 = _mm_add_epi16(cb2_16, cb3_16);
    let cr2_16 = _mm_add_epi16(cr2_16, cr3_16);

    /* Rescale Y' to Y, pack it to 8bit values and save it */
    let y1_16 = rescale_y!(param, y1_16);
    let y2_16 = rescale_y!(param, y2_16);

    let y_val = _mm_packus_epi16(y1_16, y2_16);
    let y_val = _mm_unpackhi_epi8(_mm_slli_si128(y_val, 8), y_val);

    let y_ptr1_1 = &mut buffer_y[y_index1 + 16] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(y_ptr1_1, y_val);

    /* same for the second line, compute Y', (B-Y') and (R-Y'), in 16bits values */
    /* Y is saved for each pixel, while only sums of (B-Y') and (R-Y') for pairs of adjacents
     * pixels are added to the previous values*/

    let (y1_16, cb4_16, cr4_16) = r_g_b_hi_to_y16_u16_v16!(param, col_r_1, col_g_1, col_b_1);

    let cb2_16 = _mm_add_epi16(cb2_16, cb4_16);
    let cr2_16 = _mm_add_epi16(cr2_16, cr4_16);

    let (y2_16, cb5_16, cr5_16) = r_g_b_hi_to_y16_u16_v16!(param, col_r_2, col_g_2, col_b_2);

    let cb2_16 = _mm_add_epi16(cb2_16, cb5_16);
    let cr2_16 = _mm_add_epi16(cr2_16, cr5_16);

    /* Rescale Y' to Y, pack it to 8bit values and save it */
    let y1_16 = rescale_y!(param, y1_16);
    let y2_16 = rescale_y!(param, y2_16);

    let y_val = _mm_packus_epi16(y1_16, y2_16);
    let y_val = _mm_unpackhi_epi8(_mm_slli_si128(y_val, 8), y_val);

    let y_ptr2_1 = &mut buffer_y[y_index2 + 16] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(y_ptr2_1, y_val);

    /* Rescale Cb and Cr to their final range */
    let cb2_16 = _mm_srai_epi16(cb2_16, 2);
    let cr2_16 = _mm_srai_epi16(cr2_16, 2);

    let (cb2_16, cr2_16) = rescale_uv!(param, cb2_16, cr2_16);

    /* Pack and save Cb Cr */
    let cb = _mm_packus_epi16(cb1_16, cb2_16);
    let cr = _mm_packus_epi16(cr1_16, cr2_16);

    let u_ptr = &mut buffer_u[u_index] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let v_ptr = &mut buffer_v[v_index] as *mut u8 as *mut std::arch::x86_64::__m128i;

    _mm_storeu_si128(u_ptr, cb);
    _mm_storeu_si128(v_ptr, cr);
}

#[allow(clippy::too_many_arguments)]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn rgba_to_yuv420_ssse3(
    width: usize,
    height: usize,
    buffer_rgba: &[u8],
    rgba_stride: usize,
    buffer_y: &mut [u8],
    buffer_u: &mut [u8],
    buffer_v: &mut [u8],
    y_stride: usize,
    u_stride: usize,
    v_stride: usize,
    yuv_type: YuvType,
) {
    let param = get_rgb_to_yuv_param(yuv_type);

    for y in (0..height - 1).step_by(2) {
        let mut rgba_index1 = y * rgba_stride;
        let mut rgba_index2 = (y + 1) * rgba_stride;

        let mut y_index1 = y * y_stride;
        let mut y_index2 = (y + 1) * y_stride;

        let mut u_index = (y / 2) * u_stride;
        let mut v_index = (y / 2) * v_stride;

        for _ in (0..width - 31).step_by(32) {
            unsafe {
                rgba_to_yuv420_step(
                    &param,
                    buffer_rgba,
                    buffer_y,
                    buffer_u,
                    buffer_v,
                    rgba_index1,
                    rgba_index2,
                    y_index1,
                    y_index2,
                    u_index,
                    v_index,
                );
            }
            rgba_index1 += 128;
            rgba_index2 += 128;
            y_index1 += 32;
            y_index2 += 32;
            u_index += 16;
            v_index += 16;
        }

        // Complete image width
        let cur_width = (width / 32) * 32;
        for _ in (cur_width..width - 1).step_by(2) {
            // compute yuv for the four pixels, u and v values are summed
            let mut y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index1] as u16
                + param.g_factor as u16 * buffer_rgba[rgba_index1 + 1] as u16
                + param.b_factor as u16 * buffer_rgba[rgba_index1 + 2] as u16)
                >> 8;
            let mut u_tmp = buffer_rgba[rgba_index1 + 2] as u16 - y_tmp;
            let mut v_tmp = buffer_rgba[rgba_index1] as u16 - y_tmp;
            buffer_y[y_index1] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index1 + 4] as u16
                + param.g_factor as u16 * buffer_rgba[rgba_index1 + 5] as u16
                + param.b_factor as u16 * buffer_rgba[rgba_index1 + 6] as u16)
                >> 8;
            u_tmp += buffer_rgba[rgba_index1 + 6] as u16 - y_tmp;
            v_tmp += buffer_rgba[rgba_index1 + 4] as u16 - y_tmp;
            buffer_y[y_index1 + 1] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index2] as u16
                + param.g_factor as u16 * buffer_rgba[rgba_index2 + 1] as u16
                + param.b_factor as u16 * buffer_rgba[rgba_index2 + 2] as u16)
                >> 8;
            u_tmp += buffer_rgba[rgba_index2 + 2] as u16 - y_tmp;
            v_tmp += buffer_rgba[rgba_index2] as u16 - y_tmp;
            buffer_y[y_index2] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index2 + 4] as u16
                + param.g_factor as u16 * buffer_rgba[rgba_index2 + 5] as u16
                + param.b_factor as u16 * buffer_rgba[rgba_index2 + 6] as u16)
                >> 8;
            u_tmp += buffer_rgba[rgba_index2 + 6] as u16 - y_tmp;
            v_tmp += buffer_rgba[rgba_index2 + 4] as u16 - y_tmp;
            buffer_y[y_index2 + 1] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            buffer_u[u_index] = ((((u_tmp >> 2) * param.cb_factor as u16) >> 8) + 128) as u8;
            buffer_v[v_index] = ((((v_tmp >> 2) * param.cb_factor as u16) >> 8) + 128) as u8;

            rgba_index1 += 8;
            rgba_index2 += 8;
            y_index1 += 2;
            y_index2 += 2;
            u_index += 1;
            v_index += 1;
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn rgba_to_yuv444_step(
    param: &RgbToYuvParam,
    buffer_rgba: &[u8],
    buffer_y: &mut [u8],
    buffer_u: &mut [u8],
    buffer_v: &mut [u8],
    rgba_index1: usize,
    rgba_index2: usize,
    y_index1: usize,
    y_index2: usize,
    u_index1: usize,
    u_index2: usize,
    v_index1: usize,
    v_index2: usize,
) {
    let (rgba1, rgba2, rgba3, rgba4, rgba5, rgba6, rgba7, rgba8) =
        load_rgba_4_x_2!(buffer_rgba, rgba_index1, rgba_index2);

    /*
    unpack rgb24 data to r, g and b data in separate channels see rgb.txt to
    get an idea of the algorithm, note that we only go to the next to last
    step here, because averaging in horizontal direction is easier like this
    The last step is applied further on the Y channel only
     */

    let (col_r_1, col_g_1, col_b_1, _alpha1, col_r_2, col_g_2, col_b_2, _alpha2) =
        rgb32_to_r_g_b_a!(rgba1, rgba2, rgba3, rgba4, rgba5, rgba6, rgba7, rgba8);

    /*
    first compute Y', (B-Y') and (R-Y'), in 16bits values, for the first line
    Y is saved for each pixel, while only sums of (B-Y') and (R-Y') for pairs
    of adjacents pixels are saved
     */
    let (y1_16, cb1_16, cr1_16) = r_g_b_lo_to_y16_u16_v16!(param, col_r_1, col_g_1, col_b_1);
    let (y2_16, cb2_16, cr2_16) = r_g_b_lo_to_y16_u16_v16!(param, col_r_2, col_g_2, col_b_2);

    /* Rescale Y' to Y, pack it to 8bit values and save it */
    let y1_16 = rescale_y!(param, y1_16);
    let y2_16 = rescale_y!(param, y2_16);

    let y_val = _mm_packus_epi16(y1_16, y2_16);
    let y_val = _mm_unpackhi_epi8(_mm_slli_si128(y_val, 8), y_val);

    let y_ptr1_0 = &mut buffer_y[y_index1] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(y_ptr1_0, y_val);
    /* same for the second line, compute Y', (B-Y') and (R-Y'), in 16bits values */
    /* Y is saved for each pixel, while only sums of (B-Y') and (R-Y') for pairs of adjacents
     * pixels are added to the previous values*/
    let (y1_16, cb3_16, cr3_16) = r_g_b_hi_to_y16_u16_v16!(param, col_r_1, col_g_1, col_b_1);
    let (y2_16, cb4_16, cr4_16) = r_g_b_hi_to_y16_u16_v16!(param, col_r_2, col_g_2, col_b_2);

    /* Rescale Y' to Y, pack it to 8bit values and save it */
    let y1_16 = rescale_y!(param, y1_16);
    let y2_16 = rescale_y!(param, y2_16);

    let y_val = _mm_packus_epi16(y1_16, y2_16);
    let y_val = _mm_unpackhi_epi8(_mm_slli_si128(y_val, 8), y_val);

    let y_ptr2_0 = &mut buffer_y[y_index2] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(y_ptr2_0, y_val);

    /* Rescale Cb and Cr to their final range */
    let (cb1_16, cr1_16) = rescale_uv!(param, cb1_16, cr1_16);
    let (cb2_16, cr2_16) = rescale_uv!(param, cb2_16, cr2_16);
    let (cb3_16, cr3_16) = rescale_uv!(param, cb3_16, cr3_16);
    let (cb4_16, cr4_16) = rescale_uv!(param, cb4_16, cr4_16);

    let cb = _mm_packus_epi16(cb1_16, cb2_16);
    let cb = _mm_unpackhi_epi8(_mm_slli_si128(cb, 8), cb);
    let cr = _mm_packus_epi16(cr1_16, cr2_16);
    let cr = _mm_unpackhi_epi8(_mm_slli_si128(cr, 8), cr);

    let u_ptr1_0 = &mut buffer_u[u_index1] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(u_ptr1_0, cb);

    let v_ptr1_0 = &mut buffer_v[v_index1] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(v_ptr1_0, cr);

    let cb = _mm_packus_epi16(cb3_16, cb4_16);
    let cb = _mm_unpackhi_epi8(_mm_slli_si128(cb, 8), cb);
    let cr = _mm_packus_epi16(cr3_16, cr4_16);
    let cr = _mm_unpackhi_epi8(_mm_slli_si128(cr, 8), cr);

    let u_ptr2_0 = &mut buffer_u[u_index2] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(u_ptr2_0, cb);

    let v_ptr2_0 = &mut buffer_v[v_index2] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(v_ptr2_0, cr);

    /* do the same again with next data */

    let (rgba1, rgba2, rgba3, rgba4, rgba5, rgba6, rgba7, rgba8) =
        load_rgba_4_x_2!(buffer_rgba, rgba_index1 + 64, rgba_index2 + 64);

    /* unpack rgb24 data to r, g and b data in separate channels*/
    /* see rgb.txt to get an idea of the algorithm, note that we only go to the next to last
     * step*/
    /* here, because averaging in horizontal direction is easier like this*/
    /* The last step is applied further on the Y channel only*/

    let (col_r_1, col_g_1, col_b_1, _alpha1, col_r_2, col_g_2, col_b_2, _alpha2) =
        rgb32_to_r_g_b_a!(rgba1, rgba2, rgba3, rgba4, rgba5, rgba6, rgba7, rgba8);

    /* first compute Y', (B-Y') and (R-Y'), in 16bits values, for the first line */
    /* Y is saved for each pixel, while only sums of (B-Y') and (R-Y') for pairs of adjacents
     * pixels are saved*/
    let (y1_16, cb1_16, cr1_16) = r_g_b_lo_to_y16_u16_v16!(param, col_r_1, col_g_1, col_b_1);
    let (y2_16, cb2_16, cr2_16) = r_g_b_lo_to_y16_u16_v16!(param, col_r_2, col_g_2, col_b_2);

    /* Rescale Y' to Y, pack it to 8bit values and save it */
    let y1_16 = rescale_y!(param, y1_16);
    let y2_16 = rescale_y!(param, y2_16);

    let y_val = _mm_packus_epi16(y1_16, y2_16);
    let y_val = _mm_unpackhi_epi8(_mm_slli_si128(y_val, 8), y_val);

    let y_ptr1_1 = &mut buffer_y[y_index1 + 16] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(y_ptr1_1, y_val);

    /* same for the second line, compute Y', (B-Y') and (R-Y'), in 16bits values */
    /* Y is saved for each pixel, while only sums of (B-Y') and (R-Y') for pairs of adjacents
     * pixels are added to the previous values*/
    let (y1_16, cb3_16, cr3_16) = r_g_b_hi_to_y16_u16_v16!(param, col_r_1, col_g_1, col_b_1);
    let (y2_16, cb4_16, cr4_16) = r_g_b_hi_to_y16_u16_v16!(param, col_r_2, col_g_2, col_b_2);

    /* Rescale Y' to Y, pack it to 8bit values and save it */
    let y1_16 = rescale_y!(param, y1_16);
    let y2_16 = rescale_y!(param, y2_16);

    let y_val = _mm_packus_epi16(y1_16, y2_16);
    let y_val = _mm_unpackhi_epi8(_mm_slli_si128(y_val, 8), y_val);

    let y_ptr2_1 = &mut buffer_y[y_index2 + 16] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(y_ptr2_1, y_val);
    /* Rescale Cb and Cr to their final range */
    let (cb1_16, cr1_16) = rescale_uv!(param, cb1_16, cr1_16);
    let (cb2_16, cr2_16) = rescale_uv!(param, cb2_16, cr2_16);
    let (cb3_16, cr3_16) = rescale_uv!(param, cb3_16, cr3_16);
    let (cb4_16, cr4_16) = rescale_uv!(param, cb4_16, cr4_16);

    /* Pack and save Cb Cr */
    let cb = _mm_packus_epi16(cb1_16, cb2_16);
    let cb = _mm_unpackhi_epi8(_mm_slli_si128(cb, 8), cb);
    let cr = _mm_packus_epi16(cr1_16, cr2_16);
    let cr = _mm_unpackhi_epi8(_mm_slli_si128(cr, 8), cr);

    let u_ptr1_1 = &mut buffer_u[u_index1 + 16] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(u_ptr1_1, cb);

    let v_ptr1_1 = &mut buffer_v[v_index1 + 16] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(v_ptr1_1, cr);

    /* Pack and save Cb Cr */
    let cb = _mm_packus_epi16(cb3_16, cb4_16);
    let cb = _mm_unpackhi_epi8(_mm_slli_si128(cb, 8), cb);
    let cr = _mm_packus_epi16(cr3_16, cr4_16);
    let cr = _mm_unpackhi_epi8(_mm_slli_si128(cr, 8), cr);

    let u_ptr2_1 = &mut buffer_u[u_index2 + 16] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(u_ptr2_1, cb);

    let v_ptr2_1 = &mut buffer_v[v_index2 + 16] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(v_ptr2_1, cr);
}

#[allow(clippy::too_many_arguments)]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn rgba_to_yuv444_ssse3(
    width: usize,
    height: usize,
    buffer_rgba: &[u8],
    rgba_stride: usize,
    buffer_y: &mut [u8],
    buffer_u: &mut [u8],
    buffer_v: &mut [u8],
    y_stride: usize,
    u_stride: usize,
    v_stride: usize,
    yuv_type: YuvType,
) {
    let param = get_rgb_to_yuv_param(yuv_type);

    for y in (0..height - 1).step_by(2) {
        let mut rgba_index1 = y * rgba_stride;
        let mut rgba_index2 = (y + 1) * rgba_stride;

        let mut y_index1 = y * y_stride;
        let mut y_index2 = (y + 1) * y_stride;

        let mut u_index1 = y * u_stride;
        let mut v_index1 = y * v_stride;

        let mut u_index2 = (y + 1) * u_stride;
        let mut v_index2 = (y + 1) * v_stride;

        for _ in (0..width - 31).step_by(32) {
            unsafe {
                rgba_to_yuv444_step(
                    &param,
                    buffer_rgba,
                    buffer_y,
                    buffer_u,
                    buffer_v,
                    rgba_index1,
                    rgba_index2,
                    y_index1,
                    y_index2,
                    u_index1,
                    u_index2,
                    v_index1,
                    v_index2,
                );
            }
            rgba_index1 += 128;
            rgba_index2 += 128;
            y_index1 += 32;
            y_index2 += 32;
            u_index1 += 32;
            v_index1 += 32;
            u_index2 += 32;
            v_index2 += 32;
        }

        // Complete image width
        let cur_width = (width / 32) * 32;
        for _ in cur_width..width {
            // line 1
            let y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index1] as u16
                + param.g_factor as u16 * buffer_rgba[rgba_index1 + 1] as u16
                + param.b_factor as u16 * buffer_rgba[rgba_index1 + 2] as u16)
                >> 8;
            let u_tmp = buffer_rgba[rgba_index1 + 2] as u16 - y_tmp;
            let v_tmp = buffer_rgba[rgba_index1] as u16 - y_tmp;
            buffer_y[y_index1] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            buffer_u[u_index1] = (((u_tmp * param.cb_factor as u16) >> 8) + 128) as u8;
            buffer_v[v_index1] = (((v_tmp * param.cb_factor as u16) >> 8) + 128) as u8;

            // line 1
            let y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index2] as u16
                + param.g_factor as u16 * buffer_rgba[rgba_index2 + 1] as u16
                + param.b_factor as u16 * buffer_rgba[rgba_index2 + 2] as u16)
                >> 8;
            let u_tmp = buffer_rgba[rgba_index2 + 2] as u16 - y_tmp;
            let v_tmp = buffer_rgba[rgba_index2] as u16 - y_tmp;
            buffer_y[y_index2] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            buffer_u[u_index2] = (((u_tmp * param.cb_factor as u16) >> 8) + 128) as u8;
            buffer_v[v_index2] = (((v_tmp * param.cb_factor as u16) >> 8) + 128) as u8;

            rgba_index1 += 4;
            rgba_index2 += 4;
            y_index1 += 1;
            u_index1 += 1;
            v_index1 += 1;
            y_index2 += 1;
            u_index2 += 1;
            v_index2 += 1;
        }
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn uv_to_rgb_16(
    param: &YuvToRgbParam,
    u: __m128i,
    v: __m128i,
) -> (__m128i, __m128i, __m128i, __m128i, __m128i, __m128i) {
    let r_tmp = _mm_srai_epi16(
        _mm_mullo_epi16(v, _mm_set1_epi16(param.cr_factor as i16)),
        6,
    );
    let g_tmp = _mm_srai_epi16(
        _mm_add_epi16(
            _mm_mullo_epi16(u, _mm_set1_epi16(param.g_cb_factor as i16)),
            _mm_mullo_epi16(v, _mm_set1_epi16(param.g_cr_factor as i16)),
        ),
        7,
    );
    let b_tmp = _mm_srai_epi16(
        _mm_mullo_epi16(u, _mm_set1_epi16(param.cb_factor as i16)),
        6,
    );
    let r1 = _mm_unpacklo_epi16(r_tmp, r_tmp);
    let g1 = _mm_unpacklo_epi16(g_tmp, g_tmp);
    let b1 = _mm_unpacklo_epi16(b_tmp, b_tmp);
    let r2 = _mm_unpackhi_epi16(r_tmp, r_tmp);
    let g2 = _mm_unpackhi_epi16(g_tmp, g_tmp);
    let b2 = _mm_unpackhi_epi16(b_tmp, b_tmp);

    (r1, g1, b1, r2, g2, b2)
}

#[allow(clippy::too_many_arguments)]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn add_y_to_rgb_16(
    param: &YuvToRgbParam,
    y1: __m128i,
    y2: __m128i,
    r1: __m128i,
    g1: __m128i,
    b1: __m128i,
    r2: __m128i,
    g2: __m128i,
    b2: __m128i,
) -> (__m128i, __m128i, __m128i, __m128i, __m128i, __m128i) {
    let y1 = _mm_srai_epi16(
        _mm_mullo_epi16(y1, _mm_set1_epi16(param.y_factor as i16)),
        7,
    );
    let y2 = _mm_srai_epi16(
        _mm_mullo_epi16(y2, _mm_set1_epi16(param.y_factor as i16)),
        7,
    );

    let r1 = _mm_add_epi16(y1, r1);
    let g1 = _mm_sub_epi16(y1, g1);
    let b1 = _mm_add_epi16(y1, b1);
    let r2 = _mm_add_epi16(y2, r2);
    let g2 = _mm_sub_epi16(y2, g2);
    let b2 = _mm_add_epi16(y2, b2);
    (r1, g1, b1, r2, g2, b2)
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn pack_rgb24_32_step(
    rs1: __m128i,
    rs2: __m128i,
    rs3: __m128i,
    rs4: __m128i,
    rs5: __m128i,
    rs6: __m128i,
) -> (__m128i, __m128i, __m128i, __m128i, __m128i, __m128i) {
    let rd1 = _mm_packus_epi16(
        _mm_and_si128(rs1, _mm_set1_epi16(0xff)),
        _mm_and_si128(rs2, _mm_set1_epi16(0xff)),
    );
    let rd2 = _mm_packus_epi16(
        _mm_and_si128(rs3, _mm_set1_epi16(0xff)),
        _mm_and_si128(rs4, _mm_set1_epi16(0xff)),
    );
    let rd3 = _mm_packus_epi16(
        _mm_and_si128(rs5, _mm_set1_epi16(0xff)),
        _mm_and_si128(rs6, _mm_set1_epi16(0xff)),
    );
    let rd4 = _mm_packus_epi16(_mm_srli_epi16(rs1, 8), _mm_srli_epi16(rs2, 8));
    let rd5 = _mm_packus_epi16(_mm_srli_epi16(rs3, 8), _mm_srli_epi16(rs4, 8));
    let rd6 = _mm_packus_epi16(_mm_srli_epi16(rs5, 8), _mm_srli_epi16(rs6, 8));
    (rd1, rd2, rd3, rd4, rd5, rd6)
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn pack_rgb24_32(
    r1: __m128i,
    r2: __m128i,
    g1: __m128i,
    g2: __m128i,
    b1: __m128i,
    b2: __m128i,
) -> (__m128i, __m128i, __m128i, __m128i, __m128i, __m128i) {
    let (rgb1, rgb2, rgb3, rgb4, rgb5, rgb6) = pack_rgb24_32_step(r1, r2, g1, g2, b1, b2);
    let (r1, r2, g1, g2, b1, b2) = pack_rgb24_32_step(rgb1, rgb2, rgb3, rgb4, rgb5, rgb6);
    let (rgb1, rgb2, rgb3, rgb4, rgb5, rgb6) = pack_rgb24_32_step(r1, r2, g1, g2, b1, b2);
    let (r1, r2, g1, g2, b1, b2) = pack_rgb24_32_step(rgb1, rgb2, rgb3, rgb4, rgb5, rgb6);
    let (rgb1, rgb2, rgb3, rgb4, rgb5, rgb6) = pack_rgb24_32_step(r1, r2, g1, g2, b1, b2);
    (rgb1, rgb2, rgb3, rgb4, rgb5, rgb6)
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
macro_rules! rgb_to_rgba_step {
    (
        $rs1: ident,
        $rs2: ident,
        $rs3: ident,
        $rs4: ident,
        $rs5: ident,
        $rs6: ident,
        $rs7: ident,
        $rs8: ident
    ) => {{
        let rd1 = _mm_unpacklo_epi8($rs1, $rs5);
        let rd2 = _mm_unpackhi_epi8($rs1, $rs5);
        let rd3 = _mm_unpacklo_epi8($rs2, $rs6);
        let rd4 = _mm_unpackhi_epi8($rs2, $rs6);
        let rd5 = _mm_unpacklo_epi8($rs3, $rs7);
        let rd6 = _mm_unpackhi_epi8($rs3, $rs7);
        let rd7 = _mm_unpacklo_epi8($rs4, $rs8);
        let rd8 = _mm_unpackhi_epi8($rs4, $rs8);
        (rd1, rd2, rd3, rd4, rd5, rd6, rd7, rd8)
    }};
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
macro_rules! pack_r_g_b_a_to_rgb32 {
    (
        $rs1: ident,
        $rs2: ident,
        $rs3: ident,
        $rs4: ident,
        $rs5: ident,
        $rs6: ident,
        $rs7: ident,
        $rs8: ident
    ) => {{
        let (rd1, rd2, rd3, rd4, rd5, rd6, rd7, rd8) =
            rgb_to_rgba_step!($rs1, $rs2, $rs3, $rs4, $rs5, $rs6, $rs7, $rs8);
        rgb_to_rgba_step!(rd1, rd2, rd3, rd4, rd5, rd6, rd7, rd8)
    }};
}

#[allow(clippy::too_many_arguments)]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn yuv420_to_rgb_step(
    param: &YuvToRgbParam,
    buffer_rgb: &mut [u8],
    buffer_y: &[u8],
    buffer_u: &[u8],
    buffer_v: &[u8],
    rgb_index1: usize,
    rgb_index2: usize,
    y_index1: usize,
    y_index2: usize,
    u_index1: usize,
    v_index1: usize,
) {
    let y_ptr1_0 = &buffer_y[y_index1] as *const u8 as *const std::arch::x86_64::__m128i;
    let y_ptr2_0 = &buffer_y[y_index2] as *const u8 as *const std::arch::x86_64::__m128i;
    let y_ptr1_1 = &buffer_y[y_index1 + 16] as *const u8 as *const std::arch::x86_64::__m128i;
    let y_ptr2_1 = &buffer_y[y_index2 + 16] as *const u8 as *const std::arch::x86_64::__m128i;

    let u_ptr1 = &buffer_u[u_index1] as *const u8 as *const std::arch::x86_64::__m128i;
    let v_ptr1 = &buffer_v[v_index1] as *const u8 as *const std::arch::x86_64::__m128i;

    let u = _mm_loadu_si128(u_ptr1);
    let v = _mm_loadu_si128(v_ptr1);

    let u = _mm_add_epi8(u, _mm_set1_epi8(-128));
    let v = _mm_add_epi8(v, _mm_set1_epi8(-128));

    /* process first 16 pixels of first line */
    let u_16 = _mm_srai_epi16(_mm_unpacklo_epi8(u, u), 8);
    let v_16 = _mm_srai_epi16(_mm_unpacklo_epi8(v, v), 8);

    let (r_uv_16_1, g_uv_16_1, b_uv_16_1, r_uv_16_2, g_uv_16_2, b_uv_16_2) =
        uv_to_rgb_16(param, u_16, v_16);
    let r_16_1 = r_uv_16_1;
    let g_16_1 = g_uv_16_1;
    let b_16_1 = b_uv_16_1;
    let r_16_2 = r_uv_16_2;
    let g_16_2 = g_uv_16_2;
    let b_16_2 = b_uv_16_2;

    let y = _mm_loadu_si128(y_ptr1_0);
    let y = _mm_sub_epi8(y, _mm_set1_epi8(param.y_offset as i8));
    let y_16_1 = _mm_unpacklo_epi8(y, _mm_setzero_si128());
    let y_16_2 = _mm_unpackhi_epi8(y, _mm_setzero_si128());

    let (r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2) = add_y_to_rgb_16(
        param, y_16_1, y_16_2, r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2,
    );

    let r_8_11 = _mm_packus_epi16(r_16_1, r_16_2);
    let g_8_11 = _mm_packus_epi16(g_16_1, g_16_2);
    let b_8_11 = _mm_packus_epi16(b_16_1, b_16_2);

    /* process first 16 pixels of second line */
    let r_16_1 = r_uv_16_1;
    let g_16_1 = g_uv_16_1;
    let b_16_1 = b_uv_16_1;
    let r_16_2 = r_uv_16_2;
    let g_16_2 = g_uv_16_2;
    let b_16_2 = b_uv_16_2;

    let y = _mm_loadu_si128(y_ptr2_0);
    let y = _mm_sub_epi8(y, _mm_set1_epi8(param.y_offset as i8));
    let y_16_1 = _mm_unpacklo_epi8(y, _mm_setzero_si128());
    let y_16_2 = _mm_unpackhi_epi8(y, _mm_setzero_si128());

    let (r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2) = add_y_to_rgb_16(
        param, y_16_1, y_16_2, r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2,
    );

    let r_8_21 = _mm_packus_epi16(r_16_1, r_16_2);
    let g_8_21 = _mm_packus_epi16(g_16_1, g_16_2);
    let b_8_21 = _mm_packus_epi16(b_16_1, b_16_2);

    /* process last 16 pixels of first line */
    let u_16 = _mm_srai_epi16(_mm_unpackhi_epi8(u, u), 8);
    let v_16 = _mm_srai_epi16(_mm_unpackhi_epi8(v, v), 8);

    let (r_uv_16_1, g_uv_16_1, b_uv_16_1, r_uv_16_2, g_uv_16_2, b_uv_16_2) =
        uv_to_rgb_16(param, u_16, v_16);
    let r_16_1 = r_uv_16_1;
    let g_16_1 = g_uv_16_1;
    let b_16_1 = b_uv_16_1;
    let r_16_2 = r_uv_16_2;
    let g_16_2 = g_uv_16_2;
    let b_16_2 = b_uv_16_2;

    let y = _mm_loadu_si128(y_ptr1_1);
    let y = _mm_sub_epi8(y, _mm_set1_epi8(param.y_offset as i8));
    let y_16_1 = _mm_unpacklo_epi8(y, _mm_setzero_si128());
    let y_16_2 = _mm_unpackhi_epi8(y, _mm_setzero_si128());

    let (r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2) = add_y_to_rgb_16(
        param, y_16_1, y_16_2, r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2,
    );

    let r_8_12 = _mm_packus_epi16(r_16_1, r_16_2);
    let g_8_12 = _mm_packus_epi16(g_16_1, g_16_2);
    let b_8_12 = _mm_packus_epi16(b_16_1, b_16_2);

    /* process last 16 pixels of second line */
    let r_16_1 = r_uv_16_1;
    let g_16_1 = g_uv_16_1;
    let b_16_1 = b_uv_16_1;
    let r_16_2 = r_uv_16_2;
    let g_16_2 = g_uv_16_2;
    let b_16_2 = b_uv_16_2;

    let y = _mm_loadu_si128(y_ptr2_1);
    let y = _mm_sub_epi8(y, _mm_set1_epi8(param.y_offset as i8));
    let y_16_1 = _mm_unpacklo_epi8(y, _mm_setzero_si128());
    let y_16_2 = _mm_unpackhi_epi8(y, _mm_setzero_si128());

    let (r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2) = add_y_to_rgb_16(
        param, y_16_1, y_16_2, r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2,
    );

    let r_8_22 = _mm_packus_epi16(r_16_1, r_16_2);
    let g_8_22 = _mm_packus_epi16(g_16_1, g_16_2);
    let b_8_22 = _mm_packus_epi16(b_16_1, b_16_2);

    let rgb_ptr1 = &mut buffer_rgb[rgb_index1] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr2 = &mut buffer_rgb[rgb_index1 + 16] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr3 = &mut buffer_rgb[rgb_index1 + 32] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr4 = &mut buffer_rgb[rgb_index1 + 48] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr5 = &mut buffer_rgb[rgb_index1 + 64] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr6 = &mut buffer_rgb[rgb_index1 + 80] as *mut u8 as *mut std::arch::x86_64::__m128i;

    let (rgb_1, rgb_2, rgb_3, rgb_4, rgb_5, rgb_6) =
        pack_rgb24_32(r_8_11, r_8_12, g_8_11, g_8_12, b_8_11, b_8_12);
    _mm_storeu_si128(rgb_ptr1, rgb_1);
    _mm_storeu_si128(rgb_ptr2, rgb_2);
    _mm_storeu_si128(rgb_ptr3, rgb_3);
    _mm_storeu_si128(rgb_ptr4, rgb_4);
    _mm_storeu_si128(rgb_ptr5, rgb_5);
    _mm_storeu_si128(rgb_ptr6, rgb_6);

    let rgb_ptr1 = &mut buffer_rgb[rgb_index2] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr2 = &mut buffer_rgb[rgb_index2 + 16] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr3 = &mut buffer_rgb[rgb_index2 + 32] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr4 = &mut buffer_rgb[rgb_index2 + 48] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr5 = &mut buffer_rgb[rgb_index2 + 64] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr6 = &mut buffer_rgb[rgb_index2 + 80] as *mut u8 as *mut std::arch::x86_64::__m128i;

    let (rgb_1, rgb_2, rgb_3, rgb_4, rgb_5, rgb_6) =
        pack_rgb24_32(r_8_21, r_8_22, g_8_21, g_8_22, b_8_21, b_8_22);
    _mm_storeu_si128(rgb_ptr1, rgb_1);
    _mm_storeu_si128(rgb_ptr2, rgb_2);
    _mm_storeu_si128(rgb_ptr3, rgb_3);
    _mm_storeu_si128(rgb_ptr4, rgb_4);
    _mm_storeu_si128(rgb_ptr5, rgb_5);
    _mm_storeu_si128(rgb_ptr6, rgb_6);
}

#[allow(clippy::too_many_arguments)]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn yuv420_to_rgba_step(
    param: &YuvToRgbParam,
    buffer_rgb: &mut [u8],
    buffer_y: &[u8],
    buffer_u: &[u8],
    buffer_v: &[u8],
    rgb_index1: usize,
    rgb_index2: usize,
    y_index1: usize,
    y_index2: usize,
    u_index1: usize,
    v_index1: usize,
) {
    let y_ptr1_0 = &buffer_y[y_index1] as *const u8 as *const std::arch::x86_64::__m128i;
    let y_ptr2_0 = &buffer_y[y_index2] as *const u8 as *const std::arch::x86_64::__m128i;
    let y_ptr1_1 = &buffer_y[y_index1 + 16] as *const u8 as *const std::arch::x86_64::__m128i;
    let y_ptr2_1 = &buffer_y[y_index2 + 16] as *const u8 as *const std::arch::x86_64::__m128i;

    let u_ptr1 = &buffer_u[u_index1] as *const u8 as *const std::arch::x86_64::__m128i;
    let v_ptr1 = &buffer_v[v_index1] as *const u8 as *const std::arch::x86_64::__m128i;

    let u = _mm_loadu_si128(u_ptr1);
    let v = _mm_loadu_si128(v_ptr1);

    let u = _mm_add_epi8(u, _mm_set1_epi8(-128));
    let v = _mm_add_epi8(v, _mm_set1_epi8(-128));

    /* process first 16 pixels of first line */
    let u_16 = _mm_srai_epi16(_mm_unpacklo_epi8(u, u), 8);
    let v_16 = _mm_srai_epi16(_mm_unpacklo_epi8(v, v), 8);

    let (r_uv_16_1, g_uv_16_1, b_uv_16_1, r_uv_16_2, g_uv_16_2, b_uv_16_2) =
        uv_to_rgb_16(param, u_16, v_16);
    let r_16_1 = r_uv_16_1;
    let g_16_1 = g_uv_16_1;
    let b_16_1 = b_uv_16_1;
    let r_16_2 = r_uv_16_2;
    let g_16_2 = g_uv_16_2;
    let b_16_2 = b_uv_16_2;

    let y = _mm_loadu_si128(y_ptr1_0);
    let y = _mm_sub_epi8(y, _mm_set1_epi8(param.y_offset as i8));
    let y_16_1 = _mm_unpacklo_epi8(y, _mm_setzero_si128());
    let y_16_2 = _mm_unpackhi_epi8(y, _mm_setzero_si128());

    let (r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2) = add_y_to_rgb_16(
        param, y_16_1, y_16_2, r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2,
    );

    let r_8_11 = _mm_packus_epi16(r_16_1, r_16_2);
    let g_8_11 = _mm_packus_epi16(g_16_1, g_16_2);
    let b_8_11 = _mm_packus_epi16(b_16_1, b_16_2);

    /* process first 16 pixels of second line */
    let r_16_1 = r_uv_16_1;
    let g_16_1 = g_uv_16_1;
    let b_16_1 = b_uv_16_1;
    let r_16_2 = r_uv_16_2;
    let g_16_2 = g_uv_16_2;
    let b_16_2 = b_uv_16_2;

    let y = _mm_loadu_si128(y_ptr2_0);
    let y = _mm_sub_epi8(y, _mm_set1_epi8(param.y_offset as i8));
    let y_16_1 = _mm_unpacklo_epi8(y, _mm_setzero_si128());
    let y_16_2 = _mm_unpackhi_epi8(y, _mm_setzero_si128());

    let (r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2) = add_y_to_rgb_16(
        param, y_16_1, y_16_2, r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2,
    );

    let r_8_21 = _mm_packus_epi16(r_16_1, r_16_2);
    let g_8_21 = _mm_packus_epi16(g_16_1, g_16_2);
    let b_8_21 = _mm_packus_epi16(b_16_1, b_16_2);

    /* process last 16 pixels of first line */
    let u_16 = _mm_srai_epi16(_mm_unpackhi_epi8(u, u), 8);
    let v_16 = _mm_srai_epi16(_mm_unpackhi_epi8(v, v), 8);

    let (r_uv_16_1, g_uv_16_1, b_uv_16_1, r_uv_16_2, g_uv_16_2, b_uv_16_2) =
        uv_to_rgb_16(param, u_16, v_16);
    let r_16_1 = r_uv_16_1;
    let g_16_1 = g_uv_16_1;
    let b_16_1 = b_uv_16_1;
    let r_16_2 = r_uv_16_2;
    let g_16_2 = g_uv_16_2;
    let b_16_2 = b_uv_16_2;

    let y = _mm_loadu_si128(y_ptr1_1);
    let y = _mm_sub_epi8(y, _mm_set1_epi8(param.y_offset as i8));
    let y_16_1 = _mm_unpacklo_epi8(y, _mm_setzero_si128());
    let y_16_2 = _mm_unpackhi_epi8(y, _mm_setzero_si128());

    let (r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2) = add_y_to_rgb_16(
        param, y_16_1, y_16_2, r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2,
    );

    let r_8_12 = _mm_packus_epi16(r_16_1, r_16_2);
    let g_8_12 = _mm_packus_epi16(g_16_1, g_16_2);
    let b_8_12 = _mm_packus_epi16(b_16_1, b_16_2);

    /* process last 16 pixels of second line */
    let r_16_1 = r_uv_16_1;
    let g_16_1 = g_uv_16_1;
    let b_16_1 = b_uv_16_1;
    let r_16_2 = r_uv_16_2;
    let g_16_2 = g_uv_16_2;
    let b_16_2 = b_uv_16_2;

    let y = _mm_loadu_si128(y_ptr2_1);
    let y = _mm_sub_epi8(y, _mm_set1_epi8(param.y_offset as i8));
    let y_16_1 = _mm_unpacklo_epi8(y, _mm_setzero_si128());
    let y_16_2 = _mm_unpackhi_epi8(y, _mm_setzero_si128());

    let (r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2) = add_y_to_rgb_16(
        param, y_16_1, y_16_2, r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2,
    );

    let r_8_22 = _mm_packus_epi16(r_16_1, r_16_2);
    let g_8_22 = _mm_packus_epi16(g_16_1, g_16_2);
    let b_8_22 = _mm_packus_epi16(b_16_1, b_16_2);

    let rgb_ptr1 = &mut buffer_rgb[rgb_index1] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr2 = &mut buffer_rgb[rgb_index1 + 16] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr3 = &mut buffer_rgb[rgb_index1 + 32] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr4 = &mut buffer_rgb[rgb_index1 + 48] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr5 = &mut buffer_rgb[rgb_index1 + 64] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr6 = &mut buffer_rgb[rgb_index1 + 80] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr7 = &mut buffer_rgb[rgb_index1 + 96] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr8 = &mut buffer_rgb[rgb_index1 + 112] as *mut u8 as *mut std::arch::x86_64::__m128i;

    let a_8_11 = _mm_set1_epi16(0xFF);
    let a_8_12 = _mm_set1_epi16(0xFF);

    let (rgb_1, rgb_2, rgb_3, rgb_4, rgb_5, rgb_6, rgb_7, rgb_8) =
        pack_r_g_b_a_to_rgb32!(r_8_11, r_8_12, g_8_11, g_8_12, b_8_11, b_8_12, a_8_11, a_8_12);
    _mm_storeu_si128(rgb_ptr1, rgb_1);
    _mm_storeu_si128(rgb_ptr2, rgb_2);
    _mm_storeu_si128(rgb_ptr3, rgb_3);
    _mm_storeu_si128(rgb_ptr4, rgb_4);
    _mm_storeu_si128(rgb_ptr5, rgb_5);
    _mm_storeu_si128(rgb_ptr6, rgb_6);
    _mm_storeu_si128(rgb_ptr7, rgb_7);
    _mm_storeu_si128(rgb_ptr8, rgb_8);

    let rgb_ptr1 = &mut buffer_rgb[rgb_index2] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr2 = &mut buffer_rgb[rgb_index2 + 16] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr3 = &mut buffer_rgb[rgb_index2 + 32] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr4 = &mut buffer_rgb[rgb_index2 + 48] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr5 = &mut buffer_rgb[rgb_index2 + 64] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr6 = &mut buffer_rgb[rgb_index2 + 80] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr7 = &mut buffer_rgb[rgb_index2 + 96] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr8 = &mut buffer_rgb[rgb_index2 + 112] as *mut u8 as *mut std::arch::x86_64::__m128i;

    let a_8_21 = _mm_set1_epi16(0xFF);
    let a_8_22 = _mm_set1_epi16(0xFF);

    let (rgb_1, rgb_2, rgb_3, rgb_4, rgb_5, rgb_6, rgb_7, rgb_8) =
        pack_r_g_b_a_to_rgb32!(r_8_21, r_8_22, g_8_21, g_8_22, b_8_21, b_8_22, a_8_21, a_8_22);
    _mm_storeu_si128(rgb_ptr1, rgb_1);
    _mm_storeu_si128(rgb_ptr2, rgb_2);
    _mm_storeu_si128(rgb_ptr3, rgb_3);
    _mm_storeu_si128(rgb_ptr4, rgb_4);
    _mm_storeu_si128(rgb_ptr5, rgb_5);
    _mm_storeu_si128(rgb_ptr6, rgb_6);
    _mm_storeu_si128(rgb_ptr7, rgb_7);
    _mm_storeu_si128(rgb_ptr8, rgb_8);
}

#[allow(clippy::too_many_arguments)]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn yuv420_to_rgb_ssse3(
    width: usize,
    height: usize,
    buffer_y: &[u8],
    buffer_u: &[u8],
    buffer_v: &[u8],
    y_stride: usize,
    uv_stride: usize,
    buffer_rgb: &mut [u8],
    rgb_stride: usize,
    yuv_type: YuvType,
) {
    let param = get_yuv_to_rgb_param(yuv_type);
    for y in (0..height - 1).step_by(2) {
        let mut rgb_index1 = y * rgb_stride;
        let mut rgb_index2 = (y + 1) * rgb_stride;

        let mut y_index1 = y * y_stride;
        let mut y_index2 = (y + 1) * y_stride;

        let mut u_index = (y / 2) * uv_stride;
        let mut v_index = (y / 2) * uv_stride;

        for _ in (0..width - 31).step_by(32) {
            unsafe {
                yuv420_to_rgb_step(
                    &param, buffer_rgb, buffer_y, buffer_u, buffer_v, rgb_index1, rgb_index2,
                    y_index1, y_index2, u_index, v_index,
                );
            }
            rgb_index1 += 96;
            rgb_index2 += 96;
            y_index1 += 32;
            y_index2 += 32;
            u_index += 16;
            v_index += 16;
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn yuv420_to_rgba_ssse3(
    width: usize,
    height: usize,
    buffer_y: &[u8],
    buffer_u: &[u8],
    buffer_v: &[u8],
    y_stride: usize,
    u_stride: usize,
    v_stride: usize,
    buffer_rgba: &mut [u8],
    rgba_stride: usize,
    yuv_type: YuvType,
) {
    let param = get_yuv_to_rgb_param(yuv_type);
    for y in (0..height - 1).step_by(2) {
        let mut rgba_index1 = y * rgba_stride;
        let mut rgba_index2 = (y + 1) * rgba_stride;

        let mut y_index1 = y * y_stride;
        let mut y_index2 = (y + 1) * y_stride;

        let mut u_index = (y / 2) * u_stride;
        let mut v_index = (y / 2) * v_stride;

        for _ in (0..width - 31).step_by(32) {
            unsafe {
                yuv420_to_rgba_step(
                    &param,
                    buffer_rgba,
                    buffer_y,
                    buffer_u,
                    buffer_v,
                    rgba_index1,
                    rgba_index2,
                    y_index1,
                    y_index2,
                    u_index,
                    v_index,
                );
            }
            rgba_index1 += 128;
            rgba_index2 += 128;
            y_index1 += 32;
            y_index2 += 32;
            u_index += 16;
            v_index += 16;
        }

        // Complete image width
        let cur_width = (width / 32) * 32;
        for _ in (cur_width..width - 1).step_by(2) {
            let u_tmp = buffer_u[u_index] as i16 - 128;
            let v_tmp = buffer_v[v_index] as i16 - 128;

            let b_cb_offset = (param.cb_factor as i16 * u_tmp) >> 6;
            let r_cr_offset = (param.cr_factor as i16 * v_tmp) >> 6;
            let g_cbcr_offset =
                (param.g_cb_factor as i16 * u_tmp + param.g_cr_factor as i16 * v_tmp) >> 7;

            let y_tmp =
                (param.y_factor as i16 * (buffer_y[y_index1] as i16 - param.y_offset as i16)) >> 7;
            buffer_rgba[rgba_index1] = clamp(y_tmp + r_cr_offset);
            buffer_rgba[rgba_index1 + 1] = clamp(y_tmp - g_cbcr_offset);
            buffer_rgba[rgba_index1 + 2] = clamp(y_tmp + b_cb_offset);

            let y_tmp = (param.y_factor as i16
                * (buffer_y[y_index1 + 1] as i16 - param.y_offset as i16))
                >> 7;
            buffer_rgba[rgba_index1 + 4] = clamp(y_tmp + r_cr_offset);
            buffer_rgba[rgba_index1 + 5] = clamp(y_tmp - g_cbcr_offset);
            buffer_rgba[rgba_index1 + 6] = clamp(y_tmp + b_cb_offset);

            let y_tmp =
                (param.y_factor as i16 * (buffer_y[y_index2] as i16 - param.y_offset as i16)) >> 7;
            buffer_rgba[rgba_index2] = clamp(y_tmp + r_cr_offset);
            buffer_rgba[rgba_index2 + 1] = clamp(y_tmp - g_cbcr_offset);
            buffer_rgba[rgba_index2 + 2] = clamp(y_tmp + b_cb_offset);

            let y_tmp = (param.y_factor as i16
                * (buffer_y[y_index2 + 1] as i16 - param.y_offset as i16))
                >> 7;
            buffer_rgba[rgba_index2 + 4] = clamp(y_tmp + r_cr_offset);
            buffer_rgba[rgba_index2 + 5] = clamp(y_tmp - g_cbcr_offset);
            buffer_rgba[rgba_index2 + 6] = clamp(y_tmp + b_cb_offset);

            rgba_index1 += 8;
            rgba_index2 += 8;
            y_index1 += 2;
            y_index2 += 2;
            u_index += 1;
            v_index += 1;
        }
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn uv_444_to_rgb_16(
    param: &YuvToRgbParam,
    u1: __m128i,
    v1: __m128i,
    u2: __m128i,
    v2: __m128i,
) -> (__m128i, __m128i, __m128i, __m128i, __m128i, __m128i) {
    let r1_tmp = _mm_srai_epi16(
        _mm_mullo_epi16(v1, _mm_set1_epi16(param.cr_factor as i16)),
        6,
    );
    let r2_tmp = _mm_srai_epi16(
        _mm_mullo_epi16(v2, _mm_set1_epi16(param.cr_factor as i16)),
        6,
    );
    let g1_tmp = _mm_srai_epi16(
        _mm_add_epi16(
            _mm_mullo_epi16(u1, _mm_set1_epi16(param.g_cb_factor as i16)),
            _mm_mullo_epi16(v1, _mm_set1_epi16(param.g_cr_factor as i16)),
        ),
        7,
    );
    let g2_tmp = _mm_srai_epi16(
        _mm_add_epi16(
            _mm_mullo_epi16(u2, _mm_set1_epi16(param.g_cb_factor as i16)),
            _mm_mullo_epi16(v2, _mm_set1_epi16(param.g_cr_factor as i16)),
        ),
        7,
    );
    let b1_tmp = _mm_srai_epi16(
        _mm_mullo_epi16(u1, _mm_set1_epi16(param.cb_factor as i16)),
        6,
    );
    let b2_tmp = _mm_srai_epi16(
        _mm_mullo_epi16(u2, _mm_set1_epi16(param.cb_factor as i16)),
        6,
    );
    let r1 = r1_tmp;
    let g1 = g1_tmp;
    let b1 = b1_tmp;
    let r2 = r2_tmp;
    let g2 = g2_tmp;
    let b2 = b2_tmp;

    (r1, g1, b1, r2, g2, b2)
}

#[allow(clippy::too_many_arguments)]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn yuv444_to_rgb_step(
    param: &YuvToRgbParam,
    buffer_rgb: &mut [u8],
    buffer_y: &[u8],
    buffer_u: &[u8],
    buffer_v: &[u8],
    rgb_index1: usize,
    rgb_index2: usize,
    y_index1: usize,
    y_index2: usize,
    u_index1: usize,
    u_index2: usize,
    v_index1: usize,
    v_index2: usize,
) {
    let u_ptr1_0 = &buffer_u[u_index1] as *const u8 as *const std::arch::x86_64::__m128i;
    let v_ptr1_0 = &buffer_v[v_index1] as *const u8 as *const std::arch::x86_64::__m128i;

    let u_ptr1_1 = &buffer_u[u_index1 + 16] as *const u8 as *const std::arch::x86_64::__m128i;
    let v_ptr1_1 = &buffer_v[v_index1 + 16] as *const u8 as *const std::arch::x86_64::__m128i;

    let u_ptr2_0 = &buffer_u[u_index2] as *const u8 as *const std::arch::x86_64::__m128i;
    let v_ptr2_0 = &buffer_v[v_index2] as *const u8 as *const std::arch::x86_64::__m128i;

    let u_ptr2_1 = &buffer_u[u_index2 + 16] as *const u8 as *const std::arch::x86_64::__m128i;
    let v_ptr2_1 = &buffer_v[v_index2 + 16] as *const u8 as *const std::arch::x86_64::__m128i;

    let u1 = _mm_loadu_si128(u_ptr1_0);
    let u2 = _mm_loadu_si128(u_ptr2_0);
    let u3 = _mm_loadu_si128(u_ptr1_1);
    let u4 = _mm_loadu_si128(u_ptr2_1);
    let v1 = _mm_loadu_si128(v_ptr1_0);
    let v2 = _mm_loadu_si128(v_ptr2_0);
    let v3 = _mm_loadu_si128(v_ptr1_1);
    let v4 = _mm_loadu_si128(v_ptr2_1);

    let u1 = _mm_add_epi8(u1, _mm_set1_epi8(-128));
    let v1 = _mm_add_epi8(v1, _mm_set1_epi8(-128));

    /* process first 16 pixels of first line */
    let u1_16 = _mm_srai_epi16(_mm_unpacklo_epi8(u1, u1), 8);
    let v1_16 = _mm_srai_epi16(_mm_unpacklo_epi8(v1, v1), 8);
    let u2_16 = _mm_srai_epi16(_mm_unpackhi_epi8(u1, u1), 8);
    let v2_16 = _mm_srai_epi16(_mm_unpackhi_epi8(v1, v1), 8);

    let (r_uv_16_1, g_uv_16_1, b_uv_16_1, r_uv_16_2, g_uv_16_2, b_uv_16_2) =
        uv_444_to_rgb_16(param, u1_16, v1_16, u2_16, v2_16);
    let r_16_1 = r_uv_16_1;
    let g_16_1 = g_uv_16_1;
    let b_16_1 = b_uv_16_1;
    let r_16_2 = r_uv_16_2;
    let g_16_2 = g_uv_16_2;
    let b_16_2 = b_uv_16_2;

    let y_ptr1_0 = &buffer_y[y_index1] as *const u8 as *const std::arch::x86_64::__m128i;
    let y = _mm_loadu_si128(y_ptr1_0);
    let y = _mm_sub_epi8(y, _mm_set1_epi8(param.y_offset as i8));
    let y_16_1 = _mm_unpacklo_epi8(y, _mm_setzero_si128());
    let y_16_2 = _mm_unpackhi_epi8(y, _mm_setzero_si128());

    let (r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2) = add_y_to_rgb_16(
        param, y_16_1, y_16_2, r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2,
    );

    let r_8_11 = _mm_packus_epi16(r_16_1, r_16_2);
    let g_8_11 = _mm_packus_epi16(g_16_1, g_16_2);
    let b_8_11 = _mm_packus_epi16(b_16_1, b_16_2);

    /* process first 16 pixels of second line */
    let u2 = _mm_add_epi8(u2, _mm_set1_epi8(-128));
    let v2 = _mm_add_epi8(v2, _mm_set1_epi8(-128));
    let u1_16 = _mm_srai_epi16(_mm_unpacklo_epi8(u2, u2), 8);
    let v1_16 = _mm_srai_epi16(_mm_unpacklo_epi8(v2, v2), 8);
    let u2_16 = _mm_srai_epi16(_mm_unpackhi_epi8(u2, u2), 8);
    let v2_16 = _mm_srai_epi16(_mm_unpackhi_epi8(v2, v2), 8);

    let (r_uv_16_1, g_uv_16_1, b_uv_16_1, r_uv_16_2, g_uv_16_2, b_uv_16_2) =
        uv_444_to_rgb_16(param, u1_16, v1_16, u2_16, v2_16);
    let r_16_1 = r_uv_16_1;
    let g_16_1 = g_uv_16_1;
    let b_16_1 = b_uv_16_1;
    let r_16_2 = r_uv_16_2;
    let g_16_2 = g_uv_16_2;
    let b_16_2 = b_uv_16_2;

    let y_ptr2_0 = &buffer_y[y_index2] as *const u8 as *const std::arch::x86_64::__m128i;
    let y = _mm_loadu_si128(y_ptr2_0);
    let y = _mm_sub_epi8(y, _mm_set1_epi8(param.y_offset as i8));
    let y_16_1 = _mm_unpacklo_epi8(y, _mm_setzero_si128());
    let y_16_2 = _mm_unpackhi_epi8(y, _mm_setzero_si128());

    let (r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2) = add_y_to_rgb_16(
        param, y_16_1, y_16_2, r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2,
    );

    let r_8_21 = _mm_packus_epi16(r_16_1, r_16_2);
    let g_8_21 = _mm_packus_epi16(g_16_1, g_16_2);
    let b_8_21 = _mm_packus_epi16(b_16_1, b_16_2);

    /* process last 16 pixels of first line */

    let u3 = _mm_add_epi8(u3, _mm_set1_epi8(-128));
    let v3 = _mm_add_epi8(v3, _mm_set1_epi8(-128));
    let u1_16 = _mm_srai_epi16(_mm_unpacklo_epi8(u3, u3), 8);
    let v1_16 = _mm_srai_epi16(_mm_unpacklo_epi8(v3, v3), 8);
    let u2_16 = _mm_srai_epi16(_mm_unpackhi_epi8(u3, u3), 8);
    let v2_16 = _mm_srai_epi16(_mm_unpackhi_epi8(v3, v3), 8);

    let (r_uv_16_1, g_uv_16_1, b_uv_16_1, r_uv_16_2, g_uv_16_2, b_uv_16_2) =
        uv_444_to_rgb_16(param, u1_16, v1_16, u2_16, v2_16);
    let r_16_1 = r_uv_16_1;
    let g_16_1 = g_uv_16_1;
    let b_16_1 = b_uv_16_1;
    let r_16_2 = r_uv_16_2;
    let g_16_2 = g_uv_16_2;
    let b_16_2 = b_uv_16_2;

    let y_ptr1_1 = &buffer_y[y_index1 + 16] as *const u8 as *const std::arch::x86_64::__m128i;
    let y = _mm_loadu_si128(y_ptr1_1);
    let y = _mm_sub_epi8(y, _mm_set1_epi8(param.y_offset as i8));
    let y_16_1 = _mm_unpacklo_epi8(y, _mm_setzero_si128());
    let y_16_2 = _mm_unpackhi_epi8(y, _mm_setzero_si128());

    let (r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2) = add_y_to_rgb_16(
        param, y_16_1, y_16_2, r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2,
    );

    let r_8_12 = _mm_packus_epi16(r_16_1, r_16_2);
    let g_8_12 = _mm_packus_epi16(g_16_1, g_16_2);
    let b_8_12 = _mm_packus_epi16(b_16_1, b_16_2);

    /* process last 16 pixels of second line */

    let u4 = _mm_add_epi8(u4, _mm_set1_epi8(-128));
    let v4 = _mm_add_epi8(v4, _mm_set1_epi8(-128));
    let u1_16 = _mm_srai_epi16(_mm_unpacklo_epi8(u4, u4), 8);
    let v1_16 = _mm_srai_epi16(_mm_unpacklo_epi8(v4, v4), 8);
    let u2_16 = _mm_srai_epi16(_mm_unpackhi_epi8(u4, u4), 8);
    let v2_16 = _mm_srai_epi16(_mm_unpackhi_epi8(v4, v4), 8);

    let (r_uv_16_1, g_uv_16_1, b_uv_16_1, r_uv_16_2, g_uv_16_2, b_uv_16_2) =
        uv_444_to_rgb_16(param, u1_16, v1_16, u2_16, v2_16);
    let r_16_1 = r_uv_16_1;
    let g_16_1 = g_uv_16_1;
    let b_16_1 = b_uv_16_1;
    let r_16_2 = r_uv_16_2;
    let g_16_2 = g_uv_16_2;
    let b_16_2 = b_uv_16_2;

    let y_ptr2_1 = &buffer_y[y_index2 + 16] as *const u8 as *const std::arch::x86_64::__m128i;
    let y = _mm_loadu_si128(y_ptr2_1);
    let y = _mm_sub_epi8(y, _mm_set1_epi8(param.y_offset as i8));
    let y_16_1 = _mm_unpacklo_epi8(y, _mm_setzero_si128());
    let y_16_2 = _mm_unpackhi_epi8(y, _mm_setzero_si128());

    let (r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2) = add_y_to_rgb_16(
        param, y_16_1, y_16_2, r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2,
    );

    let r_8_22 = _mm_packus_epi16(r_16_1, r_16_2);
    let g_8_22 = _mm_packus_epi16(g_16_1, g_16_2);
    let b_8_22 = _mm_packus_epi16(b_16_1, b_16_2);

    let (rgb_1, rgb_2, rgb_3, rgb_4, rgb_5, rgb_6) =
        pack_rgb24_32(r_8_11, r_8_12, g_8_11, g_8_12, b_8_11, b_8_12);

    let rgb_ptr1 = &mut buffer_rgb[rgb_index1] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr2 = &mut buffer_rgb[rgb_index1 + 16] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr3 = &mut buffer_rgb[rgb_index1 + 32] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr4 = &mut buffer_rgb[rgb_index1 + 48] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr5 = &mut buffer_rgb[rgb_index1 + 64] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr6 = &mut buffer_rgb[rgb_index1 + 80] as *mut u8 as *mut std::arch::x86_64::__m128i;

    _mm_storeu_si128(rgb_ptr1, rgb_1);
    _mm_storeu_si128(rgb_ptr2, rgb_2);
    _mm_storeu_si128(rgb_ptr3, rgb_3);
    _mm_storeu_si128(rgb_ptr4, rgb_4);
    _mm_storeu_si128(rgb_ptr5, rgb_5);
    _mm_storeu_si128(rgb_ptr6, rgb_6);

    let (rgb_1, rgb_2, rgb_3, rgb_4, rgb_5, rgb_6) =
        pack_rgb24_32(r_8_21, r_8_22, g_8_21, g_8_22, b_8_21, b_8_22);

    let rgb_ptr1 = &mut buffer_rgb[rgb_index2] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr2 = &mut buffer_rgb[rgb_index2 + 16] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr3 = &mut buffer_rgb[rgb_index2 + 32] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr4 = &mut buffer_rgb[rgb_index2 + 48] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr5 = &mut buffer_rgb[rgb_index2 + 64] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr6 = &mut buffer_rgb[rgb_index2 + 80] as *mut u8 as *mut std::arch::x86_64::__m128i;

    _mm_storeu_si128(rgb_ptr1, rgb_1);
    _mm_storeu_si128(rgb_ptr2, rgb_2);
    _mm_storeu_si128(rgb_ptr3, rgb_3);
    _mm_storeu_si128(rgb_ptr4, rgb_4);
    _mm_storeu_si128(rgb_ptr5, rgb_5);
    _mm_storeu_si128(rgb_ptr6, rgb_6);
}

#[allow(clippy::too_many_arguments)]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn yuv444_rgb_ssse3(
    width: usize,
    height: usize,
    buffer_y: &[u8],
    buffer_u: &[u8],
    buffer_v: &[u8],
    y_stride: usize,
    uv_stride: usize,
    buffer_rgb: &mut [u8],
    rgb_stride: usize,
    yuv_type: YuvType,
) {
    let param = get_yuv_to_rgb_param(yuv_type);
    for y in (0..height - 1).step_by(2) {
        let mut rgb_index1 = y * rgb_stride;
        let mut rgb_index2 = (y + 1) * rgb_stride;

        let mut y_index1 = y * y_stride;
        let mut y_index2 = (y + 1) * y_stride;

        let mut u_index1 = y * uv_stride;
        let mut v_index1 = y * uv_stride;
        let mut u_index2 = (y + 1) * uv_stride;
        let mut v_index2 = (y + 1) * uv_stride;

        for _ in (0..width - 31).step_by(32) {
            unsafe {
                yuv444_to_rgb_step(
                    &param, buffer_rgb, buffer_y, buffer_u, buffer_v, rgb_index1, rgb_index2,
                    y_index1, y_index2, u_index1, u_index2, v_index1, v_index2,
                );
            }
            rgb_index1 += 96;
            rgb_index2 += 96;
            y_index1 += 32;
            y_index2 += 32;
            u_index1 += 32;
            v_index1 += 32;
            u_index2 += 32;
            v_index2 += 32;
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn yuv444_to_rgba_step(
    param: &YuvToRgbParam,
    buffer_rgb: &mut [u8],
    buffer_y: &[u8],
    buffer_u: &[u8],
    buffer_v: &[u8],
    rgb_index1: usize,
    rgb_index2: usize,
    y_index1: usize,
    y_index2: usize,
    u_index1: usize,
    u_index2: usize,
    v_index1: usize,
    v_index2: usize,
) {
    let u_ptr1_0 = &buffer_u[u_index1] as *const u8 as *const std::arch::x86_64::__m128i;
    let v_ptr1_0 = &buffer_v[v_index1] as *const u8 as *const std::arch::x86_64::__m128i;

    let u_ptr1_1 = &buffer_u[u_index1 + 16] as *const u8 as *const std::arch::x86_64::__m128i;
    let v_ptr1_1 = &buffer_v[v_index1 + 16] as *const u8 as *const std::arch::x86_64::__m128i;

    let u_ptr2_0 = &buffer_u[u_index2] as *const u8 as *const std::arch::x86_64::__m128i;
    let v_ptr2_0 = &buffer_v[v_index2] as *const u8 as *const std::arch::x86_64::__m128i;

    let u_ptr2_1 = &buffer_u[u_index2 + 16] as *const u8 as *const std::arch::x86_64::__m128i;
    let v_ptr2_1 = &buffer_v[v_index2 + 16] as *const u8 as *const std::arch::x86_64::__m128i;

    let u1 = _mm_loadu_si128(u_ptr1_0);
    let u2 = _mm_loadu_si128(u_ptr2_0);
    let u3 = _mm_loadu_si128(u_ptr1_1);
    let u4 = _mm_loadu_si128(u_ptr2_1);
    let v1 = _mm_loadu_si128(v_ptr1_0);
    let v2 = _mm_loadu_si128(v_ptr2_0);
    let v3 = _mm_loadu_si128(v_ptr1_1);
    let v4 = _mm_loadu_si128(v_ptr2_1);

    let u1 = _mm_add_epi8(u1, _mm_set1_epi8(-128));
    let v1 = _mm_add_epi8(v1, _mm_set1_epi8(-128));

    /* process first 16 pixels of first line */
    let u1_16 = _mm_srai_epi16(_mm_unpacklo_epi8(u1, u1), 8);
    let v1_16 = _mm_srai_epi16(_mm_unpacklo_epi8(v1, v1), 8);
    let u2_16 = _mm_srai_epi16(_mm_unpackhi_epi8(u1, u1), 8);
    let v2_16 = _mm_srai_epi16(_mm_unpackhi_epi8(v1, v1), 8);

    let (r_uv_16_1, g_uv_16_1, b_uv_16_1, r_uv_16_2, g_uv_16_2, b_uv_16_2) =
        uv_444_to_rgb_16(param, u1_16, v1_16, u2_16, v2_16);
    let r_16_1 = r_uv_16_1;
    let g_16_1 = g_uv_16_1;
    let b_16_1 = b_uv_16_1;
    let r_16_2 = r_uv_16_2;
    let g_16_2 = g_uv_16_2;
    let b_16_2 = b_uv_16_2;

    let y_ptr1_0 = &buffer_y[y_index1] as *const u8 as *const std::arch::x86_64::__m128i;
    let y = _mm_loadu_si128(y_ptr1_0);
    let y = _mm_sub_epi8(y, _mm_set1_epi8(param.y_offset as i8));
    let y_16_1 = _mm_unpacklo_epi8(y, _mm_setzero_si128());
    let y_16_2 = _mm_unpackhi_epi8(y, _mm_setzero_si128());

    let (r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2) = add_y_to_rgb_16(
        param, y_16_1, y_16_2, r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2,
    );

    let r_8_11 = _mm_packus_epi16(r_16_1, r_16_2);
    let g_8_11 = _mm_packus_epi16(g_16_1, g_16_2);
    let b_8_11 = _mm_packus_epi16(b_16_1, b_16_2);

    /* process first 16 pixels of second line */
    let u2 = _mm_add_epi8(u2, _mm_set1_epi8(-128));
    let v2 = _mm_add_epi8(v2, _mm_set1_epi8(-128));
    let u1_16 = _mm_srai_epi16(_mm_unpacklo_epi8(u2, u2), 8);
    let v1_16 = _mm_srai_epi16(_mm_unpacklo_epi8(v2, v2), 8);
    let u2_16 = _mm_srai_epi16(_mm_unpackhi_epi8(u2, u2), 8);
    let v2_16 = _mm_srai_epi16(_mm_unpackhi_epi8(v2, v2), 8);

    let (r_uv_16_1, g_uv_16_1, b_uv_16_1, r_uv_16_2, g_uv_16_2, b_uv_16_2) =
        uv_444_to_rgb_16(param, u1_16, v1_16, u2_16, v2_16);
    let r_16_1 = r_uv_16_1;
    let g_16_1 = g_uv_16_1;
    let b_16_1 = b_uv_16_1;
    let r_16_2 = r_uv_16_2;
    let g_16_2 = g_uv_16_2;
    let b_16_2 = b_uv_16_2;

    let y_ptr2_0 = &buffer_y[y_index2] as *const u8 as *const std::arch::x86_64::__m128i;
    let y = _mm_loadu_si128(y_ptr2_0);
    let y = _mm_sub_epi8(y, _mm_set1_epi8(param.y_offset as i8));
    let y_16_1 = _mm_unpacklo_epi8(y, _mm_setzero_si128());
    let y_16_2 = _mm_unpackhi_epi8(y, _mm_setzero_si128());

    let (r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2) = add_y_to_rgb_16(
        param, y_16_1, y_16_2, r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2,
    );

    let r_8_21 = _mm_packus_epi16(r_16_1, r_16_2);
    let g_8_21 = _mm_packus_epi16(g_16_1, g_16_2);
    let b_8_21 = _mm_packus_epi16(b_16_1, b_16_2);

    /* process last 16 pixels of first line */

    let u3 = _mm_add_epi8(u3, _mm_set1_epi8(-128));
    let v3 = _mm_add_epi8(v3, _mm_set1_epi8(-128));
    let u1_16 = _mm_srai_epi16(_mm_unpacklo_epi8(u3, u3), 8);
    let v1_16 = _mm_srai_epi16(_mm_unpacklo_epi8(v3, v3), 8);
    let u2_16 = _mm_srai_epi16(_mm_unpackhi_epi8(u3, u3), 8);
    let v2_16 = _mm_srai_epi16(_mm_unpackhi_epi8(v3, v3), 8);

    let (r_uv_16_1, g_uv_16_1, b_uv_16_1, r_uv_16_2, g_uv_16_2, b_uv_16_2) =
        uv_444_to_rgb_16(param, u1_16, v1_16, u2_16, v2_16);
    let r_16_1 = r_uv_16_1;
    let g_16_1 = g_uv_16_1;
    let b_16_1 = b_uv_16_1;
    let r_16_2 = r_uv_16_2;
    let g_16_2 = g_uv_16_2;
    let b_16_2 = b_uv_16_2;

    let y_ptr1_1 = &buffer_y[y_index1 + 16] as *const u8 as *const std::arch::x86_64::__m128i;
    let y = _mm_loadu_si128(y_ptr1_1);
    let y = _mm_sub_epi8(y, _mm_set1_epi8(param.y_offset as i8));
    let y_16_1 = _mm_unpacklo_epi8(y, _mm_setzero_si128());
    let y_16_2 = _mm_unpackhi_epi8(y, _mm_setzero_si128());

    let (r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2) = add_y_to_rgb_16(
        param, y_16_1, y_16_2, r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2,
    );

    let r_8_12 = _mm_packus_epi16(r_16_1, r_16_2);
    let g_8_12 = _mm_packus_epi16(g_16_1, g_16_2);
    let b_8_12 = _mm_packus_epi16(b_16_1, b_16_2);

    /* process last 16 pixels of second line */

    let u4 = _mm_add_epi8(u4, _mm_set1_epi8(-128));
    let v4 = _mm_add_epi8(v4, _mm_set1_epi8(-128));
    let u1_16 = _mm_srai_epi16(_mm_unpacklo_epi8(u4, u4), 8);
    let v1_16 = _mm_srai_epi16(_mm_unpacklo_epi8(v4, v4), 8);
    let u2_16 = _mm_srai_epi16(_mm_unpackhi_epi8(u4, u4), 8);
    let v2_16 = _mm_srai_epi16(_mm_unpackhi_epi8(v4, v4), 8);

    let (r_uv_16_1, g_uv_16_1, b_uv_16_1, r_uv_16_2, g_uv_16_2, b_uv_16_2) =
        uv_444_to_rgb_16(param, u1_16, v1_16, u2_16, v2_16);
    let r_16_1 = r_uv_16_1;
    let g_16_1 = g_uv_16_1;
    let b_16_1 = b_uv_16_1;
    let r_16_2 = r_uv_16_2;
    let g_16_2 = g_uv_16_2;
    let b_16_2 = b_uv_16_2;

    let y_ptr2_1 = &buffer_y[y_index2 + 16] as *const u8 as *const std::arch::x86_64::__m128i;
    let y = _mm_loadu_si128(y_ptr2_1);
    let y = _mm_sub_epi8(y, _mm_set1_epi8(param.y_offset as i8));
    let y_16_1 = _mm_unpacklo_epi8(y, _mm_setzero_si128());
    let y_16_2 = _mm_unpackhi_epi8(y, _mm_setzero_si128());

    let (r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2) = add_y_to_rgb_16(
        param, y_16_1, y_16_2, r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2,
    );

    let r_8_22 = _mm_packus_epi16(r_16_1, r_16_2);
    let g_8_22 = _mm_packus_epi16(g_16_1, g_16_2);
    let b_8_22 = _mm_packus_epi16(b_16_1, b_16_2);

    let a_8_11 = _mm_set1_epi16(0xFF);
    let a_8_12 = _mm_set1_epi16(0xFF);

    let (rgb_1, rgb_2, rgb_3, rgb_4, rgb_5, rgb_6, rgb_7, rgb_8) =
        pack_r_g_b_a_to_rgb32!(r_8_11, r_8_12, g_8_11, g_8_12, b_8_11, b_8_12, a_8_11, a_8_12);

    let rgb_ptr1 = &mut buffer_rgb[rgb_index1] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr2 = &mut buffer_rgb[rgb_index1 + 16] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr3 = &mut buffer_rgb[rgb_index1 + 32] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr4 = &mut buffer_rgb[rgb_index1 + 48] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr5 = &mut buffer_rgb[rgb_index1 + 64] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr6 = &mut buffer_rgb[rgb_index1 + 80] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr7 = &mut buffer_rgb[rgb_index1 + 96] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr8 = &mut buffer_rgb[rgb_index1 + 112] as *mut u8 as *mut std::arch::x86_64::__m128i;

    _mm_storeu_si128(rgb_ptr1, rgb_1);
    _mm_storeu_si128(rgb_ptr2, rgb_2);
    _mm_storeu_si128(rgb_ptr3, rgb_3);
    _mm_storeu_si128(rgb_ptr4, rgb_4);
    _mm_storeu_si128(rgb_ptr5, rgb_5);
    _mm_storeu_si128(rgb_ptr6, rgb_6);
    _mm_storeu_si128(rgb_ptr7, rgb_7);
    _mm_storeu_si128(rgb_ptr8, rgb_8);

    let a_8_11 = _mm_set1_epi16(0xFF);
    let a_8_12 = _mm_set1_epi16(0xFF);

    let (rgb_1, rgb_2, rgb_3, rgb_4, rgb_5, rgb_6, rgb_7, rgb_8) =
        pack_r_g_b_a_to_rgb32!(r_8_21, r_8_22, g_8_21, g_8_22, b_8_21, b_8_22, a_8_11, a_8_12);

    let rgb_ptr1 = &mut buffer_rgb[rgb_index2] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr2 = &mut buffer_rgb[rgb_index2 + 16] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr3 = &mut buffer_rgb[rgb_index2 + 32] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr4 = &mut buffer_rgb[rgb_index2 + 48] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr5 = &mut buffer_rgb[rgb_index2 + 64] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr6 = &mut buffer_rgb[rgb_index2 + 80] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr7 = &mut buffer_rgb[rgb_index2 + 96] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr8 = &mut buffer_rgb[rgb_index2 + 112] as *mut u8 as *mut std::arch::x86_64::__m128i;

    _mm_storeu_si128(rgb_ptr1, rgb_1);
    _mm_storeu_si128(rgb_ptr2, rgb_2);
    _mm_storeu_si128(rgb_ptr3, rgb_3);
    _mm_storeu_si128(rgb_ptr4, rgb_4);
    _mm_storeu_si128(rgb_ptr5, rgb_5);
    _mm_storeu_si128(rgb_ptr6, rgb_6);
    _mm_storeu_si128(rgb_ptr7, rgb_7);
    _mm_storeu_si128(rgb_ptr8, rgb_8);
}

#[allow(clippy::too_many_arguments)]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn yuv444_to_rgba_ssse3(
    width: usize,
    height: usize,
    buffer_y: &[u8],
    buffer_u: &[u8],
    buffer_v: &[u8],
    y_stride: usize,
    u_stride: usize,
    v_stride: usize,
    buffer_rgba: &mut [u8],
    rgba_stride: usize,
    yuv_type: YuvType,
) {
    let param = get_yuv_to_rgb_param(yuv_type);
    for y in (0..height - 1).step_by(2) {
        let mut rgba_index1 = y * rgba_stride;
        let mut rgba_index2 = (y + 1) * rgba_stride;

        let mut y_index1 = y * y_stride;
        let mut y_index2 = (y + 1) * y_stride;

        let mut u_index1 = y * u_stride;
        let mut v_index1 = y * v_stride;
        let mut u_index2 = (y + 1) * u_stride;
        let mut v_index2 = (y + 1) * v_stride;

        for _ in (0..width - 31).step_by(32) {
            unsafe {
                yuv444_to_rgba_step(
                    &param,
                    buffer_rgba,
                    buffer_y,
                    buffer_u,
                    buffer_v,
                    rgba_index1,
                    rgba_index2,
                    y_index1,
                    y_index2,
                    u_index1,
                    u_index2,
                    v_index1,
                    v_index2,
                );
            }
            rgba_index1 += 128;
            rgba_index2 += 128;
            y_index1 += 32;
            y_index2 += 32;
            u_index1 += 32;
            v_index1 += 32;
            u_index2 += 32;
            v_index2 += 32;
        }

        // Complete image width
        let cur_width = (width / 32) * 32;
        for _ in cur_width..width {
            // line 1
            let u_tmp = buffer_u[u_index1] as i16 - 128;
            let v_tmp = buffer_v[v_index1] as i16 - 128;

            let b_cb_offset = (param.cb_factor as i16 * u_tmp) >> 6;
            let r_cr_offset = (param.cr_factor as i16 * v_tmp) >> 6;
            let g_cbcr_offset =
                (param.g_cb_factor as i16 * u_tmp + param.g_cr_factor as i16 * v_tmp) >> 7;

            let y_tmp =
                (param.y_factor as i16 * (buffer_y[y_index1] as i16 - param.y_offset as i16)) >> 7;
            buffer_rgba[rgba_index1] = clamp(y_tmp + r_cr_offset);
            buffer_rgba[rgba_index1 + 1] = clamp(y_tmp - g_cbcr_offset);
            buffer_rgba[rgba_index1 + 2] = clamp(y_tmp + b_cb_offset);

            // line 1
            let u_tmp = buffer_u[u_index2] as i16 - 128;
            let v_tmp = buffer_v[v_index2] as i16 - 128;

            let b_cb_offset = (param.cb_factor as i16 * u_tmp) >> 6;
            let r_cr_offset = (param.cr_factor as i16 * v_tmp) >> 6;
            let g_cbcr_offset =
                (param.g_cb_factor as i16 * u_tmp + param.g_cr_factor as i16 * v_tmp) >> 7;

            let y_tmp =
                (param.y_factor as i16 * (buffer_y[y_index2] as i16 - param.y_offset as i16)) >> 7;
            buffer_rgba[rgba_index2] = clamp(y_tmp + r_cr_offset);
            buffer_rgba[rgba_index2 + 1] = clamp(y_tmp - g_cbcr_offset);
            buffer_rgba[rgba_index2 + 2] = clamp(y_tmp + b_cb_offset);

            rgba_index1 += 4;
            rgba_index2 += 4;
            y_index1 += 1;
            u_index1 += 1;
            v_index1 += 1;
            y_index2 += 1;
            u_index2 += 1;
            v_index2 += 1;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn rgba_to_nv12_std(
    width: usize,
    height: usize,
    buffer_rgba: &[u8],
    rgba_stride: usize,
    buffer_y: &mut [u8],
    buffer_uv: &mut [u8],
    y_stride: usize,
    uv_stride: usize,
    yuv_type: YuvType,
) {
    let param = get_rgb_to_yuv_param(yuv_type);
    for y in (0..height - 1).step_by(2) {
        let mut y_index1 = y * y_stride;
        let mut y_index2 = (y + 1) * y_stride;

        let mut rgba_index1 = y * rgba_stride;
        let mut rgba_index2 = (y + 1) * rgba_stride;
        let mut uv_index = (y / 2) * uv_stride;
        for _ in (0..width - 1).step_by(2) {
            // compute yuv for the four pixels, u and v values are summed
            let mut y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index1] as u16
                + param.g_factor as u16 * buffer_rgba[rgba_index1 + 1] as u16
                + param.b_factor as u16 * buffer_rgba[rgba_index1 + 2] as u16)
                >> 8;
            let mut u_tmp = buffer_rgba[rgba_index1 + 2] as u16 - y_tmp;
            let mut v_tmp = buffer_rgba[rgba_index1] as u16 - y_tmp;
            buffer_y[y_index1] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index1 + 4] as u16
                + param.g_factor as u16 * buffer_rgba[rgba_index1 + 5] as u16
                + param.b_factor as u16 * buffer_rgba[rgba_index1 + 6] as u16)
                >> 8;
            u_tmp += buffer_rgba[rgba_index1 + 6] as u16 - y_tmp;
            v_tmp += buffer_rgba[rgba_index1 + 4] as u16 - y_tmp;
            buffer_y[y_index1 + 1] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index2] as u16
                + param.g_factor as u16 * buffer_rgba[rgba_index2 + 1] as u16
                + param.b_factor as u16 * buffer_rgba[rgba_index2 + 2] as u16)
                >> 8;
            u_tmp += buffer_rgba[rgba_index2 + 2] as u16 - y_tmp;
            v_tmp += buffer_rgba[rgba_index2] as u16 - y_tmp;
            buffer_y[y_index2] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index2 + 4] as u16
                + param.g_factor as u16 * buffer_rgba[rgba_index2 + 5] as u16
                + param.b_factor as u16 * buffer_rgba[rgba_index2 + 6] as u16)
                >> 8;
            u_tmp += buffer_rgba[rgba_index2 + 6] as u16 - y_tmp;
            v_tmp += buffer_rgba[rgba_index2 + 4] as u16 - y_tmp;
            buffer_y[y_index2 + 1] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            buffer_uv[uv_index] = ((((u_tmp >> 2) * param.cb_factor as u16) >> 8) + 128) as u8;
            buffer_uv[uv_index + 1] = ((((v_tmp >> 2) * param.cb_factor as u16) >> 8) + 128) as u8;

            rgba_index1 += 8;
            rgba_index2 += 8;
            y_index1 += 2;
            y_index2 += 2;
            uv_index += 2;
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn rgba_to_nv12_step(
    param: &RgbToYuvParam,
    buffer_rgba: &[u8],
    buffer_y: &mut [u8],
    buffer_uv: &mut [u8],
    rgba_index1: usize,
    rgba_index2: usize,
    y_index1: usize,
    y_index2: usize,
    uv_index: usize,
) {
    let (rgba1, rgba2, rgba3, rgba4, rgba5, rgba6, rgba7, rgba8) =
        load_rgba_4_x_2!(buffer_rgba, rgba_index1, rgba_index2);

    /* first compute Y', (B-Y') and (R-Y'), in 16bits values, for the first line
    Y is saved for each pixel, while only sums of (B-Y') and (R-Y') for pairs of adjacents
    pixels are saved */

    let (col_r_1, col_g_1, col_b_1, _alpha1, col_r_2, col_g_2, col_b_2, _alpha2) =
        rgb32_to_r_g_b_a!(rgba1, rgba2, rgba3, rgba4, rgba5, rgba6, rgba7, rgba8);

    let (y1_16, cb1_16, cr1_16) = r_g_b_lo_to_y16_u16_v16!(param, col_r_1, col_g_1, col_b_1);
    let (y2_16, cb2_16, cr2_16) = r_g_b_lo_to_y16_u16_v16!(param, col_r_2, col_g_2, col_b_2);

    let cb1_16 = _mm_add_epi16(cb1_16, cb2_16);
    let cr1_16 = _mm_add_epi16(cr1_16, cr2_16);

    /* Rescale Y' to Y, pack it to 8bit values and save it */

    let y1_16 = rescale_y!(param, y1_16);
    let y2_16 = rescale_y!(param, y2_16);
    let y_val = _mm_packus_epi16(y1_16, y2_16);
    let y_val = _mm_unpackhi_epi8(_mm_slli_si128(y_val, 8), y_val);

    let y_ptr1_0 = &mut buffer_y[y_index1] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(y_ptr1_0, y_val);

    /* same for the second line, compute Y', (B-Y') and (R-Y'), in 16bits values
    Y is saved for each pixel, while only sums of (B-Y') and (R-Y') for pairs of adjacents
    pixels are added to the previous values*/

    let (y1_16, cb3_16, cr3_16) = r_g_b_hi_to_y16_u16_v16!(param, col_r_1, col_g_1, col_b_1);

    let cb1_16 = _mm_add_epi16(cb1_16, cb3_16);
    let cr1_16 = _mm_add_epi16(cr1_16, cr3_16);

    let (y2_16, cb4_16, cr4_16) = r_g_b_hi_to_y16_u16_v16!(param, col_r_2, col_g_2, col_b_2);

    let cb1_16 = _mm_add_epi16(cb1_16, cb4_16);
    let cr1_16 = _mm_add_epi16(cr1_16, cr4_16);

    /* Rescale Y' to Y, pack it to 8bit values and save it */

    let y1_16 = rescale_y!(param, y1_16);
    let y2_16 = rescale_y!(param, y2_16);

    let y_val = _mm_packus_epi16(y1_16, y2_16);
    let y_val = _mm_unpackhi_epi8(_mm_slli_si128(y_val, 8), y_val);

    let y_ptr2_0 = &mut buffer_y[y_index2] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(y_ptr2_0, y_val);

    /* Rescale Cb and Cr to their final range */
    let cb1_16 = _mm_srai_epi16(cb1_16, 2);
    let cr1_16 = _mm_srai_epi16(cr1_16, 2);

    let (cb1_16, cr1_16) = rescale_uv!(param, cb1_16, cr1_16);

    /* do the same again with next data */

    let (rgba1, rgba2, rgba3, rgba4, rgba5, rgba6, rgba7, rgba8) =
        load_rgba_4_x_2!(buffer_rgba, rgba_index1 + 64, rgba_index2 + 64);

    /* unpack rgb24 data to r, g and b data in separate channels
       see rgb.txt to get an idea of the algorithm, note that we only go to the next to last step
       here, because averaging in horizontal direction is easier like this
       The last step is applied further on the Y channel only
    */

    let (col_r_1, col_g_1, col_b_1, _alpha1, col_r_2, col_g_2, col_b_2, _alpha2) =
        rgb32_to_r_g_b_a!(rgba1, rgba2, rgba3, rgba4, rgba5, rgba6, rgba7, rgba8);

    /* first compute Y', (B-Y') and (R-Y'), in 16bits values, for the first line
      Y is saved for each pixel, while only sums of (B-Y') and (R-Y') for pairs of adjacents
      pixels are saved
    */
    let (y1_16, cb2_16, cr2_16) = r_g_b_lo_to_y16_u16_v16!(param, col_r_1, col_g_1, col_b_1);
    let (y2_16, cb3_16, cr3_16) = r_g_b_lo_to_y16_u16_v16!(param, col_r_2, col_g_2, col_b_2);

    let cb2_16 = _mm_add_epi16(cb2_16, cb3_16);
    let cr2_16 = _mm_add_epi16(cr2_16, cr3_16);

    /* Rescale Y' to Y, pack it to 8bit values and save it */
    let y1_16 = rescale_y!(param, y1_16);
    let y2_16 = rescale_y!(param, y2_16);

    let y_val = _mm_packus_epi16(y1_16, y2_16);
    let y_val = _mm_unpackhi_epi8(_mm_slli_si128(y_val, 8), y_val);

    let y_ptr1_1 = &mut buffer_y[y_index1 + 16] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(y_ptr1_1, y_val);

    /* same for the second line, compute Y', (B-Y') and (R-Y'), in 16bits values */
    /* Y is saved for each pixel, while only sums of (B-Y') and (R-Y') for pairs of adjacents
     * pixels are added to the previous values*/

    let (y1_16, cb4_16, cr4_16) = r_g_b_hi_to_y16_u16_v16!(param, col_r_1, col_g_1, col_b_1);

    let cb2_16 = _mm_add_epi16(cb2_16, cb4_16);
    let cr2_16 = _mm_add_epi16(cr2_16, cr4_16);

    let (y2_16, cb5_16, cr5_16) = r_g_b_hi_to_y16_u16_v16!(param, col_r_2, col_g_2, col_b_2);

    let cb2_16 = _mm_add_epi16(cb2_16, cb5_16);
    let cr2_16 = _mm_add_epi16(cr2_16, cr5_16);

    /* Rescale Y' to Y, pack it to 8bit values and save it */
    let y1_16 = rescale_y!(param, y1_16);
    let y2_16 = rescale_y!(param, y2_16);

    let y_val = _mm_packus_epi16(y1_16, y2_16);
    let y_val = _mm_unpackhi_epi8(_mm_slli_si128(y_val, 8), y_val);

    let y_ptr2_1 = &mut buffer_y[y_index2 + 16] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(y_ptr2_1, y_val);

    /* Rescale Cb and Cr to their final range */
    let cb2_16 = _mm_srai_epi16(cb2_16, 2);
    let cr2_16 = _mm_srai_epi16(cr2_16, 2);

    let (cb2_16, cr2_16) = rescale_uv!(param, cb2_16, cr2_16);

    /* Pack and save Cb Cr */
    let cb = _mm_packus_epi16(cb1_16, cb2_16);
    let cr = _mm_packus_epi16(cr1_16, cr2_16);

    let cbcr1 = _mm_unpacklo_epi8(cb, cr);
    let cbcr2 = _mm_unpackhi_epi8(cb, cr);

    let uv_ptr = &mut buffer_uv[uv_index] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(uv_ptr, cbcr1);

    let uv_ptr = &mut buffer_uv[uv_index + 16] as *mut u8 as *mut std::arch::x86_64::__m128i;
    _mm_storeu_si128(uv_ptr, cbcr2);
}

#[allow(clippy::too_many_arguments)]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn rgba_to_nv12_ssse3(
    width: usize,
    height: usize,
    buffer_rgba: &[u8],
    rgba_stride: usize,
    buffer_y: &mut [u8],
    buffer_uv: &mut [u8],
    y_stride: usize,
    uv_stride: usize,
    yuv_type: YuvType,
) {
    let param = get_rgb_to_yuv_param(yuv_type);

    for y in (0..height - 1).step_by(2) {
        let mut rgba_index1 = y * rgba_stride;
        let mut rgba_index2 = (y + 1) * rgba_stride;

        let mut y_index1 = y * y_stride;
        let mut y_index2 = (y + 1) * y_stride;

        let mut uv_index = (y / 2) * uv_stride;

        for _ in (0..width - 31).step_by(32) {
            unsafe {
                rgba_to_nv12_step(
                    &param,
                    buffer_rgba,
                    buffer_y,
                    buffer_uv,
                    rgba_index1,
                    rgba_index2,
                    y_index1,
                    y_index2,
                    uv_index,
                );
            }
            rgba_index1 += 128;
            rgba_index2 += 128;
            y_index1 += 32;
            y_index2 += 32;
            uv_index += 32;
        }

        // Complete image width
        let cur_width = (width / 32) * 32;
        for _ in (cur_width..width - 1).step_by(2) {
            let mut y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index1] as u16
                + param.g_factor as u16 * buffer_rgba[rgba_index1 + 1] as u16
                + param.b_factor as u16 * buffer_rgba[rgba_index1 + 2] as u16)
                >> 8;
            let mut u_tmp = buffer_rgba[rgba_index1 + 2] as u16 - y_tmp;
            let mut v_tmp = buffer_rgba[rgba_index1] as u16 - y_tmp;
            buffer_y[y_index1] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index1 + 4] as u16
                + param.g_factor as u16 * buffer_rgba[rgba_index1 + 5] as u16
                + param.b_factor as u16 * buffer_rgba[rgba_index1 + 6] as u16)
                >> 8;
            u_tmp += buffer_rgba[rgba_index1 + 6] as u16 - y_tmp;
            v_tmp += buffer_rgba[rgba_index1 + 4] as u16 - y_tmp;
            buffer_y[y_index1 + 1] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index2] as u16
                + param.g_factor as u16 * buffer_rgba[rgba_index2 + 1] as u16
                + param.b_factor as u16 * buffer_rgba[rgba_index2 + 2] as u16)
                >> 8;
            u_tmp += buffer_rgba[rgba_index2 + 2] as u16 - y_tmp;
            v_tmp += buffer_rgba[rgba_index2] as u16 - y_tmp;
            buffer_y[y_index2] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index2 + 4] as u16
                + param.g_factor as u16 * buffer_rgba[rgba_index2 + 5] as u16
                + param.b_factor as u16 * buffer_rgba[rgba_index2 + 6] as u16)
                >> 8;
            u_tmp += buffer_rgba[rgba_index2 + 6] as u16 - y_tmp;
            v_tmp += buffer_rgba[rgba_index2 + 4] as u16 - y_tmp;
            buffer_y[y_index2 + 1] =
                (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

            buffer_uv[uv_index] = ((((u_tmp >> 2) * param.cb_factor as u16) >> 8) + 128) as u8;
            buffer_uv[uv_index + 1] = ((((v_tmp >> 2) * param.cb_factor as u16) >> 8) + 128) as u8;

            rgba_index1 += 8;
            rgba_index2 += 8;
            y_index1 += 2;
            y_index2 += 2;
            uv_index += 2;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn nv12_rgba_std(
    width: usize,
    height: usize,
    buffer_y: &[u8],
    buffer_uv: &[u8],
    y_stride: usize,
    uv_stride: usize,
    buffer_rgba: &mut [u8],
    rgba_stride: usize,
    yuv_type: YuvType,
) {
    debug!(
        "tttt {}x{} {} {} {}",
        width, height, y_stride, uv_stride, rgba_stride
    );
    let param = get_yuv_to_rgb_param(yuv_type);
    for y in (0..height - 1).step_by(2) {
        let mut rgba_index1 = y * rgba_stride;
        let mut rgba_index2 = (y + 1) * rgba_stride;

        let mut uv_index = (y / 2) * uv_stride;
        let mut y_index1 = y * y_stride;
        let mut y_index2 = (y + 1) * y_stride;
        for _ in (0..width - 1).step_by(2) {
            let u_tmp = buffer_uv[uv_index] as i16 - 128;
            let v_tmp = buffer_uv[uv_index + 1] as i16 - 128;

            let b_cb_offset = (param.cb_factor as i16 * u_tmp) >> 6;
            let r_cr_offset = (param.cr_factor as i16 * v_tmp) >> 6;
            let g_cbcr_offset =
                (param.g_cb_factor as i16 * u_tmp + param.g_cr_factor as i16 * v_tmp) >> 7;

            let y_tmp =
                (param.y_factor as i16 * (buffer_y[y_index1] as i16 - param.y_offset as i16)) >> 7;
            buffer_rgba[rgba_index1] = clamp(y_tmp + r_cr_offset);
            buffer_rgba[rgba_index1 + 1] = clamp(y_tmp - g_cbcr_offset);
            buffer_rgba[rgba_index1 + 2] = clamp(y_tmp + b_cb_offset);

            let y_tmp = (param.y_factor as i16
                * (buffer_y[y_index1 + 1] as i16 - param.y_offset as i16))
                >> 7;
            buffer_rgba[rgba_index1 + 4] = clamp(y_tmp + r_cr_offset);
            buffer_rgba[rgba_index1 + 5] = clamp(y_tmp - g_cbcr_offset);
            buffer_rgba[rgba_index1 + 6] = clamp(y_tmp + b_cb_offset);

            let y_tmp =
                (param.y_factor as i16 * (buffer_y[y_index2] as i16 - param.y_offset as i16)) >> 7;
            buffer_rgba[rgba_index2] = clamp(y_tmp + r_cr_offset);
            buffer_rgba[rgba_index2 + 1] = clamp(y_tmp - g_cbcr_offset);
            buffer_rgba[rgba_index2 + 2] = clamp(y_tmp + b_cb_offset);

            let y_tmp = (param.y_factor as i16
                * (buffer_y[y_index2 + 1] as i16 - param.y_offset as i16))
                >> 7;
            buffer_rgba[rgba_index2 + 4] = clamp(y_tmp + r_cr_offset);
            buffer_rgba[rgba_index2 + 5] = clamp(y_tmp - g_cbcr_offset);
            buffer_rgba[rgba_index2 + 6] = clamp(y_tmp + b_cb_offset);

            rgba_index1 += 8;
            rgba_index2 += 8;
            y_index1 += 2;
            y_index2 += 2;
            uv_index += 2;
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn nv12_to_rgba_step(
    param: &YuvToRgbParam,
    buffer_rgba: &mut [u8],
    buffer_y: &[u8],
    buffer_uv: &[u8],
    rgba_index1: usize,
    rgba_index2: usize,
    y_index1: usize,
    y_index2: usize,
    uv_index: usize,
) {
    let y_ptr1_0 = &buffer_y[y_index1] as *const u8 as *const std::arch::x86_64::__m128i;
    let y_ptr2_0 = &buffer_y[y_index2] as *const u8 as *const std::arch::x86_64::__m128i;
    let y_ptr1_1 = &buffer_y[y_index1 + 16] as *const u8 as *const std::arch::x86_64::__m128i;
    let y_ptr2_1 = &buffer_y[y_index2 + 16] as *const u8 as *const std::arch::x86_64::__m128i;

    let uv_ptr1 = &buffer_uv[uv_index] as *const u8 as *const std::arch::x86_64::__m128i;
    let uv_ptr2 = &buffer_uv[uv_index + 16] as *const u8 as *const std::arch::x86_64::__m128i;

    let uv1 = _mm_loadu_si128(uv_ptr1);
    let uv2 = _mm_loadu_si128(uv_ptr2);

    let u = _mm_packus_epi16(
        _mm_and_si128(uv1, _mm_set1_epi16(255)),
        _mm_and_si128(uv2, _mm_set1_epi16(255)),
    );
    let uv1 = _mm_srli_epi16(uv1, 8);
    let uv2 = _mm_srli_epi16(uv2, 8);
    let v = _mm_packus_epi16(
        _mm_and_si128(uv1, _mm_set1_epi16(255)),
        _mm_and_si128(uv2, _mm_set1_epi16(255)),
    );

    let u = _mm_add_epi8(u, _mm_set1_epi8(-128));
    let v = _mm_add_epi8(v, _mm_set1_epi8(-128));

    /* process first 16 pixels of first line */
    let u_16 = _mm_srai_epi16(_mm_unpacklo_epi8(u, u), 8);
    let v_16 = _mm_srai_epi16(_mm_unpacklo_epi8(v, v), 8);

    let (r_uv_16_1, g_uv_16_1, b_uv_16_1, r_uv_16_2, g_uv_16_2, b_uv_16_2) =
        uv_to_rgb_16(param, u_16, v_16);
    let r_16_1 = r_uv_16_1;
    let g_16_1 = g_uv_16_1;
    let b_16_1 = b_uv_16_1;
    let r_16_2 = r_uv_16_2;
    let g_16_2 = g_uv_16_2;
    let b_16_2 = b_uv_16_2;

    let y = _mm_loadu_si128(y_ptr1_0);
    let y = _mm_sub_epi8(y, _mm_set1_epi8(param.y_offset as i8));
    let y_16_1 = _mm_unpacklo_epi8(y, _mm_setzero_si128());
    let y_16_2 = _mm_unpackhi_epi8(y, _mm_setzero_si128());

    let (r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2) = add_y_to_rgb_16(
        param, y_16_1, y_16_2, r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2,
    );

    let r_8_11 = _mm_packus_epi16(r_16_1, r_16_2);
    let g_8_11 = _mm_packus_epi16(g_16_1, g_16_2);
    let b_8_11 = _mm_packus_epi16(b_16_1, b_16_2);

    /* process first 16 pixels of second line */
    let r_16_1 = r_uv_16_1;
    let g_16_1 = g_uv_16_1;
    let b_16_1 = b_uv_16_1;
    let r_16_2 = r_uv_16_2;
    let g_16_2 = g_uv_16_2;
    let b_16_2 = b_uv_16_2;

    let y = _mm_loadu_si128(y_ptr2_0);
    let y = _mm_sub_epi8(y, _mm_set1_epi8(param.y_offset as i8));
    let y_16_1 = _mm_unpacklo_epi8(y, _mm_setzero_si128());
    let y_16_2 = _mm_unpackhi_epi8(y, _mm_setzero_si128());

    let (r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2) = add_y_to_rgb_16(
        param, y_16_1, y_16_2, r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2,
    );

    let r_8_21 = _mm_packus_epi16(r_16_1, r_16_2);
    let g_8_21 = _mm_packus_epi16(g_16_1, g_16_2);
    let b_8_21 = _mm_packus_epi16(b_16_1, b_16_2);

    /* process last 16 pixels of first line */
    let u_16 = _mm_srai_epi16(_mm_unpackhi_epi8(u, u), 8);
    let v_16 = _mm_srai_epi16(_mm_unpackhi_epi8(v, v), 8);

    let (r_uv_16_1, g_uv_16_1, b_uv_16_1, r_uv_16_2, g_uv_16_2, b_uv_16_2) =
        uv_to_rgb_16(param, u_16, v_16);
    let r_16_1 = r_uv_16_1;
    let g_16_1 = g_uv_16_1;
    let b_16_1 = b_uv_16_1;
    let r_16_2 = r_uv_16_2;
    let g_16_2 = g_uv_16_2;
    let b_16_2 = b_uv_16_2;

    let y = _mm_loadu_si128(y_ptr1_1);
    let y = _mm_sub_epi8(y, _mm_set1_epi8(param.y_offset as i8));
    let y_16_1 = _mm_unpacklo_epi8(y, _mm_setzero_si128());
    let y_16_2 = _mm_unpackhi_epi8(y, _mm_setzero_si128());

    let (r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2) = add_y_to_rgb_16(
        param, y_16_1, y_16_2, r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2,
    );

    let r_8_12 = _mm_packus_epi16(r_16_1, r_16_2);
    let g_8_12 = _mm_packus_epi16(g_16_1, g_16_2);
    let b_8_12 = _mm_packus_epi16(b_16_1, b_16_2);

    /* process last 16 pixels of second line */
    let r_16_1 = r_uv_16_1;
    let g_16_1 = g_uv_16_1;
    let b_16_1 = b_uv_16_1;
    let r_16_2 = r_uv_16_2;
    let g_16_2 = g_uv_16_2;
    let b_16_2 = b_uv_16_2;

    let y = _mm_loadu_si128(y_ptr2_1);
    let y = _mm_sub_epi8(y, _mm_set1_epi8(param.y_offset as i8));
    let y_16_1 = _mm_unpacklo_epi8(y, _mm_setzero_si128());
    let y_16_2 = _mm_unpackhi_epi8(y, _mm_setzero_si128());

    let (r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2) = add_y_to_rgb_16(
        param, y_16_1, y_16_2, r_16_1, g_16_1, b_16_1, r_16_2, g_16_2, b_16_2,
    );

    let r_8_22 = _mm_packus_epi16(r_16_1, r_16_2);
    let g_8_22 = _mm_packus_epi16(g_16_1, g_16_2);
    let b_8_22 = _mm_packus_epi16(b_16_1, b_16_2);

    let rgb_ptr1 = &mut buffer_rgba[rgba_index1] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr2 = &mut buffer_rgba[rgba_index1 + 16] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr3 = &mut buffer_rgba[rgba_index1 + 32] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr4 = &mut buffer_rgba[rgba_index1 + 48] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr5 = &mut buffer_rgba[rgba_index1 + 64] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr6 = &mut buffer_rgba[rgba_index1 + 80] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr7 = &mut buffer_rgba[rgba_index1 + 96] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr8 =
        &mut buffer_rgba[rgba_index1 + 112] as *mut u8 as *mut std::arch::x86_64::__m128i;

    let a_8_11 = _mm_set1_epi16(0xFF);
    let a_8_12 = _mm_set1_epi16(0xFF);

    let (rgb_1, rgb_2, rgb_3, rgb_4, rgb_5, rgb_6, rgb_7, rgb_8) =
        pack_r_g_b_a_to_rgb32!(r_8_11, r_8_12, g_8_11, g_8_12, b_8_11, b_8_12, a_8_11, a_8_12);
    _mm_storeu_si128(rgb_ptr1, rgb_1);
    _mm_storeu_si128(rgb_ptr2, rgb_2);
    _mm_storeu_si128(rgb_ptr3, rgb_3);
    _mm_storeu_si128(rgb_ptr4, rgb_4);
    _mm_storeu_si128(rgb_ptr5, rgb_5);
    _mm_storeu_si128(rgb_ptr6, rgb_6);
    _mm_storeu_si128(rgb_ptr7, rgb_7);
    _mm_storeu_si128(rgb_ptr8, rgb_8);

    let rgb_ptr1 = &mut buffer_rgba[rgba_index2] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr2 = &mut buffer_rgba[rgba_index2 + 16] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr3 = &mut buffer_rgba[rgba_index2 + 32] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr4 = &mut buffer_rgba[rgba_index2 + 48] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr5 = &mut buffer_rgba[rgba_index2 + 64] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr6 = &mut buffer_rgba[rgba_index2 + 80] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr7 = &mut buffer_rgba[rgba_index2 + 96] as *mut u8 as *mut std::arch::x86_64::__m128i;
    let rgb_ptr8 =
        &mut buffer_rgba[rgba_index2 + 112] as *mut u8 as *mut std::arch::x86_64::__m128i;

    let a_8_21 = _mm_set1_epi16(0xFF);
    let a_8_22 = _mm_set1_epi16(0xFF);

    let (rgb_1, rgb_2, rgb_3, rgb_4, rgb_5, rgb_6, rgb_7, rgb_8) =
        pack_r_g_b_a_to_rgb32!(r_8_21, r_8_22, g_8_21, g_8_22, b_8_21, b_8_22, a_8_21, a_8_22);
    _mm_storeu_si128(rgb_ptr1, rgb_1);
    _mm_storeu_si128(rgb_ptr2, rgb_2);
    _mm_storeu_si128(rgb_ptr3, rgb_3);
    _mm_storeu_si128(rgb_ptr4, rgb_4);
    _mm_storeu_si128(rgb_ptr5, rgb_5);
    _mm_storeu_si128(rgb_ptr6, rgb_6);
    _mm_storeu_si128(rgb_ptr7, rgb_7);
    _mm_storeu_si128(rgb_ptr8, rgb_8);
}

#[allow(clippy::too_many_arguments)]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn nv12_rgba_ssse3(
    width: usize,
    height: usize,
    buffer_y: &[u8],
    buffer_uv: &[u8],
    y_stride: usize,
    uv_stride: usize,
    buffer_rgba: &mut [u8],
    rgba_stride: usize,
    yuv_type: YuvType,
) {
    let param = get_yuv_to_rgb_param(yuv_type);
    for y in (0..height - 1).step_by(2) {
        let mut rgba_index1 = y * rgba_stride;
        let mut rgba_index2 = (y + 1) * rgba_stride;

        let mut y_index1 = y * y_stride;
        let mut y_index2 = (y + 1) * y_stride;

        let mut uv_index = (y / 2) * uv_stride;

        for _ in (0..width - 31).step_by(32) {
            unsafe {
                nv12_to_rgba_step(
                    &param,
                    buffer_rgba,
                    buffer_y,
                    buffer_uv,
                    rgba_index1,
                    rgba_index2,
                    y_index1,
                    y_index2,
                    uv_index,
                );
            }
            rgba_index1 += 128;
            rgba_index2 += 128;
            y_index1 += 32;
            y_index2 += 32;
            uv_index += 32;
        }
        // Complete image width
        let cur_width = (width / 32) * 32;
        for _ in (cur_width..width - 1).step_by(2) {
            let u_tmp = buffer_uv[uv_index] as i16 - 128;
            let v_tmp = buffer_uv[uv_index + 1] as i16 - 128;

            let b_cb_offset = (param.cb_factor as i16 * u_tmp) >> 6;
            let r_cr_offset = (param.cr_factor as i16 * v_tmp) >> 6;
            let g_cbcr_offset =
                (param.g_cb_factor as i16 * u_tmp + param.g_cr_factor as i16 * v_tmp) >> 7;

            let y_tmp =
                (param.y_factor as i16 * (buffer_y[y_index1] as i16 - param.y_offset as i16)) >> 7;
            buffer_rgba[rgba_index1] = clamp(y_tmp + r_cr_offset);
            buffer_rgba[rgba_index1 + 1] = clamp(y_tmp - g_cbcr_offset);
            buffer_rgba[rgba_index1 + 2] = clamp(y_tmp + b_cb_offset);

            let y_tmp = (param.y_factor as i16
                * (buffer_y[y_index1 + 1] as i16 - param.y_offset as i16))
                >> 7;
            buffer_rgba[rgba_index1 + 4] = clamp(y_tmp + r_cr_offset);
            buffer_rgba[rgba_index1 + 5] = clamp(y_tmp - g_cbcr_offset);
            buffer_rgba[rgba_index1 + 6] = clamp(y_tmp + b_cb_offset);

            let y_tmp =
                (param.y_factor as i16 * (buffer_y[y_index2] as i16 - param.y_offset as i16)) >> 7;
            buffer_rgba[rgba_index2] = clamp(y_tmp + r_cr_offset);
            buffer_rgba[rgba_index2 + 1] = clamp(y_tmp - g_cbcr_offset);
            buffer_rgba[rgba_index2 + 2] = clamp(y_tmp + b_cb_offset);

            let y_tmp = (param.y_factor as i16
                * (buffer_y[y_index2 + 1] as i16 - param.y_offset as i16))
                >> 7;
            buffer_rgba[rgba_index2 + 4] = clamp(y_tmp + r_cr_offset);
            buffer_rgba[rgba_index2 + 5] = clamp(y_tmp - g_cbcr_offset);
            buffer_rgba[rgba_index2 + 6] = clamp(y_tmp + b_cb_offset);

            rgba_index1 += 8;
            rgba_index2 += 8;
            y_index1 += 2;
            y_index2 += 2;
            uv_index += 2;
        }
    }
}

#[allow(clippy::explicit_counter_loop)]
#[allow(clippy::too_many_arguments)]
pub fn rgba_to_yuv420_std_rayon(
    width: usize,
    height: usize,
    buffer_rgba: &[u8],
    rgba_stride: usize,
    buffer_y: &mut [u8],
    buffer_u: &mut [u8],
    buffer_v: &mut [u8],
    y_stride: usize,
    u_stride: usize,
    v_stride: usize,
    yuv_type: YuvType,
) {
    let buffer_y_raw = buffer_y.as_mut_ptr() as *mut u8;
    let buffer_u_raw = buffer_u.as_mut_ptr() as *mut u8;
    let buffer_v_raw = buffer_v.as_mut_ptr() as *mut u8;

    let slice_y_len = y_stride;
    let slice_u_len = u_stride;
    let slice_v_len = v_stride;

    let mut slices = vec![];

    for y in (0..height - 1).step_by(2) {
        let y_index1 = y * y_stride;
        let y_index2 = (y + 1) * y_stride;

        let u_index = (y / 2) * u_stride;
        let v_index = (y / 2) * v_stride;

        let cur_y_ptr_1 = unsafe { buffer_y_raw.add(y_index1) };
        let cur_y_ptr_2 = unsafe { buffer_y_raw.add(y_index2) };
        let cur_y_slice_1 = unsafe { std::slice::from_raw_parts_mut(cur_y_ptr_1, slice_y_len) };
        let cur_y_slice_2 = unsafe { std::slice::from_raw_parts_mut(cur_y_ptr_2, slice_y_len) };

        let cur_u_ptr = unsafe { buffer_u_raw.add(u_index) };
        let cur_v_ptr = unsafe { buffer_v_raw.add(v_index) };
        let cur_u_slice = unsafe { std::slice::from_raw_parts_mut(cur_u_ptr, slice_u_len) };
        let cur_v_slice = unsafe { std::slice::from_raw_parts_mut(cur_v_ptr, slice_v_len) };

        slices.push((y, cur_y_slice_1, cur_y_slice_2, cur_u_slice, cur_v_slice));
    }

    let param = get_rgb_to_yuv_param(yuv_type);
    slices.par_iter_mut().for_each(
        |(y, cur_y_slice_1, cur_y_slice_2, cur_u_slice, cur_v_slice)| {
            let mut y_index1 = 0;
            let mut y_index2 = 0;

            let mut rgba_index1 = *y * rgba_stride;
            let mut rgba_index2 = (*y + 1) * rgba_stride;

            let mut u_index = 0;
            let mut v_index = 0;

            for _ in (0..width - 1).step_by(2) {
                // compute yuv for the four pixels, u and v values are summed
                let mut y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index1] as u16
                    + param.g_factor as u16 * buffer_rgba[rgba_index1 + 1] as u16
                    + param.b_factor as u16 * buffer_rgba[rgba_index1 + 2] as u16)
                    >> 8;
                let mut u_tmp = buffer_rgba[rgba_index1 + 2] as u16 - y_tmp;
                let mut v_tmp = buffer_rgba[rgba_index1] as u16 - y_tmp;
                cur_y_slice_1[y_index1] =
                    (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

                y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index1 + 4] as u16
                    + param.g_factor as u16 * buffer_rgba[rgba_index1 + 5] as u16
                    + param.b_factor as u16 * buffer_rgba[rgba_index1 + 6] as u16)
                    >> 8;
                u_tmp += buffer_rgba[rgba_index1 + 6] as u16 - y_tmp;
                v_tmp += buffer_rgba[rgba_index1 + 4] as u16 - y_tmp;
                cur_y_slice_1[y_index1 + 1] =
                    (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

                y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index2] as u16
                    + param.g_factor as u16 * buffer_rgba[rgba_index2 + 1] as u16
                    + param.b_factor as u16 * buffer_rgba[rgba_index2 + 2] as u16)
                    >> 8;
                u_tmp += buffer_rgba[rgba_index2 + 2] as u16 - y_tmp;
                v_tmp += buffer_rgba[rgba_index2] as u16 - y_tmp;
                cur_y_slice_2[y_index2] =
                    (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

                y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index2 + 4] as u16
                    + param.g_factor as u16 * buffer_rgba[rgba_index2 + 5] as u16
                    + param.b_factor as u16 * buffer_rgba[rgba_index2 + 6] as u16)
                    >> 8;
                u_tmp += buffer_rgba[rgba_index2 + 6] as u16 - y_tmp;
                v_tmp += buffer_rgba[rgba_index2 + 4] as u16 - y_tmp;
                cur_y_slice_2[y_index2 + 1] =
                    (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

                cur_u_slice[u_index] = ((((u_tmp >> 2) * param.cb_factor as u16) >> 8) + 128) as u8;
                cur_v_slice[v_index] = ((((v_tmp >> 2) * param.cb_factor as u16) >> 8) + 128) as u8;

                rgba_index1 += 8;
                rgba_index2 += 8;
                y_index1 += 2;
                y_index2 += 2;
                u_index += 1;
                v_index += 1;
            }
        },
    )
}

#[allow(clippy::explicit_counter_loop)]
#[allow(clippy::too_many_arguments)]
pub fn rgba_to_yuv444_std_rayon(
    width: usize,
    height: usize,
    buffer_rgba: &[u8],
    rgba_stride: usize,
    buffer_y: &mut [u8],
    buffer_u: &mut [u8],
    buffer_v: &mut [u8],
    y_stride: usize,
    u_stride: usize,
    v_stride: usize,
    yuv_type: YuvType,
) {
    let buffer_y_raw = buffer_y.as_mut_ptr() as *mut u8;
    let buffer_u_raw = buffer_u.as_mut_ptr() as *mut u8;
    let buffer_v_raw = buffer_v.as_mut_ptr() as *mut u8;

    let slice_y_len = y_stride;
    let slice_u_len = u_stride;
    let slice_v_len = v_stride;

    let mut slices = vec![];

    for y in 0..height {
        let y_index = y * y_stride;
        let u_index = y * u_stride;
        let v_index = y * v_stride;

        let cur_y_ptr = unsafe { buffer_y_raw.add(y_index) };
        let cur_y_slice = unsafe { std::slice::from_raw_parts_mut(cur_y_ptr, slice_y_len) };

        let cur_u_ptr = unsafe { buffer_u_raw.add(u_index) };
        let cur_v_ptr = unsafe { buffer_v_raw.add(v_index) };

        let cur_u_slice = unsafe { std::slice::from_raw_parts_mut(cur_u_ptr, slice_u_len) };
        let cur_v_slice = unsafe { std::slice::from_raw_parts_mut(cur_v_ptr, slice_v_len) };

        slices.push((y, cur_y_slice, cur_u_slice, cur_v_slice));
    }

    let param = get_rgb_to_yuv_param(yuv_type);
    slices
        .par_iter_mut()
        .for_each(|(y, cur_y_slice, cur_u_slice, cur_v_slice)| {
            let mut y_index1 = 0;

            let mut rgba_index = *y * rgba_stride;

            let mut u_index = 0;
            let mut v_index = 0;
            for _ in 0..width {
                // compute yuv for the four pixels, u and v values are summed
                let y_tmp = (param.r_factor as u16 * buffer_rgba[rgba_index] as u16
                    + param.g_factor as u16 * buffer_rgba[rgba_index + 1] as u16
                    + param.b_factor as u16 * buffer_rgba[rgba_index + 2] as u16)
                    >> 8;
                let u_tmp = buffer_rgba[rgba_index + 2] as u16 - y_tmp;
                let v_tmp = buffer_rgba[rgba_index] as u16 - y_tmp;
                cur_y_slice[y_index1] =
                    (((y_tmp * param.y_factor as u16) >> 7) + param.y_offset as u16) as u8;

                cur_u_slice[u_index] = (((u_tmp * param.cb_factor as u16) >> 8) + 128) as u8;
                cur_v_slice[v_index] = (((v_tmp * param.cb_factor as u16) >> 8) + 128) as u8;

                rgba_index += 4;
                y_index1 += 1;
                u_index += 1;
                v_index += 1;
            }
        })
}
