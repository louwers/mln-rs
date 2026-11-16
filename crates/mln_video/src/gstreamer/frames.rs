//! The frames of an appsink's samples: planes copied from memory, or a
//! dmabuf's descriptors.

use std::ffi::{CStr, c_void};
use std::os::fd::BorrowedFd;

use mln_gfx::video::{CpuPlane, DmaBufPlane, FramePixels, GpuFrame, VideoFrame, YuvColor, YuvMatrix, YuvRange};
use mln_util::size::Size;

use super::ffi::{self, Api, MiniRef};

/// The raw formats that the CPU pipelines' appsinks accept, in GStreamer's
/// caps syntax.
pub(super) const CPU_FORMATS: &str = "video/x-raw, format=(string){ NV12, I420, RGBA, RGBx }";

/// What a sample holds.
pub(super) enum Converted {
    Frame(VideoFrame),
    /// A frame in GPU memory that can't be shared as a dmabuf: the player
    /// asks for frames in memory instead.
    Unshareable(String),
}

/// The frame of `sample`.
///
/// # Errors
///
/// The sample isn't a frame of the formats that the appsink's caps allow,
/// or its buffer is shorter than its layout.
pub(super) fn convert(api: &Api, sample: &MiniRef) -> Result<Converted, String> {
    // SAFETY: the sample is live, and its caps, buffer and segment are its
    // own while it lives.
    unsafe {
        let caps = (api.sample_get_caps)(sample.as_ptr());
        let buffer = (api.sample_get_buffer)(sample.as_ptr());
        if caps.is_null() || buffer.is_null() {
            return Err("a sample without caps or a buffer".to_owned());
        }
        let structure = (api.caps_get_structure)(caps, 0);
        if structure.is_null() {
            return Err("empty caps".to_owned());
        }
        let (Some(width), Some(height)) =
            (api.structure_int(structure, c"width"), api.structure_int(structure, c"height"))
        else {
            return Err("caps without a size".to_owned());
        };
        if width <= 0 || height <= 0 {
            return Err(format!("a frame of {width}x{height}"));
        }
        let size = Size::new(width as u32, height as u32);
        let time = stream_time(api, sample.as_ptr(), buffer);
        let color = color(api, structure, size);
        let features = (api.caps_get_features)(caps, 0);
        let pixels = if !features.is_null() && (api.caps_features_contains)(features, c"memory:DMABuf".as_ptr()) != 0 {
            let drm_format = api.structure_string(structure, c"drm-format").unwrap_or_default();
            match dma_buf(api, buffer, &drm_format, color) {
                Ok(frame) => FramePixels::Gpu(frame),
                Err(why) => return Ok(Converted::Unshareable(why)),
            }
        } else {
            let format = api.structure_string(structure, c"format").unwrap_or_default();
            cpu_pixels(api, buffer, &format, size, color)?
        };
        Ok(Converted::Frame(VideoFrame { size, pixels, time }))
    }
}

/// The time of `buffer` in the video, from the sample's segment.
///
/// # Safety
///
/// `sample` is live and `buffer` is its buffer.
unsafe fn stream_time(api: &Api, sample: *mut c_void, buffer: *mut c_void) -> Option<f64> {
    // SAFETY: as the caller says.
    unsafe {
        let pts = (*buffer.cast::<ffi::Buffer>()).pts;
        let segment = (api.sample_get_segment)(sample);
        if pts == ffi::CLOCK_TIME_NONE || segment.is_null() {
            return None;
        }
        let time = (api.segment_to_stream_time)(segment, ffi::FORMAT_TIME, pts);
        (time != ffi::CLOCK_TIME_NONE).then(|| time as f64 / 1e9)
    }
}

/// How the caps' YUV samples are colors. Caps without colorimetry get
/// GStreamer's default for their height: BT.2020 for 2160 rows and more,
/// BT.709 above 576 rows, BT.601 below; in limited range.
///
/// # Safety
///
/// `structure` is a live `GstStructure`.
unsafe fn color(api: &Api, structure: *const c_void, size: Size) -> YuvColor {
    let by_height = if size.height >= 2160 {
        YuvMatrix::Bt2020
    } else if size.height > 576 {
        YuvMatrix::Bt709
    } else {
        YuvMatrix::Bt601
    };
    let mut colorimetry = ffi::VideoColorimetry::default();
    // SAFETY: as the caller says; the string outlives the call.
    let parsed = unsafe {
        api.structure_string(structure, c"colorimetry")
            .and_then(|text| std::ffi::CString::new(text).ok())
            .is_some_and(|text| (api.video_colorimetry_from_string)(&raw mut colorimetry, text.as_ptr()) != 0)
    };
    if !parsed {
        return YuvColor { matrix: by_height, range: YuvRange::Limited };
    }
    let matrix = match colorimetry.matrix {
        ffi::COLOR_MATRIX_BT709 | ffi::COLOR_MATRIX_SMPTE240M => YuvMatrix::Bt709,
        ffi::COLOR_MATRIX_BT601 | ffi::COLOR_MATRIX_FCC => YuvMatrix::Bt601,
        ffi::COLOR_MATRIX_BT2020 => YuvMatrix::Bt2020,
        _ => by_height,
    };
    let range = match colorimetry.range {
        ffi::COLOR_RANGE_0_255 => YuvRange::Full,
        // Limited, or unknown, which most video is.
        _ => YuvRange::Limited,
    };
    YuvColor { matrix, range }
}

/// Where each plane of a frame starts in its buffer, and its stride.
struct Layout {
    offsets: [usize; 4],
    strides: [usize; 4],
}

/// The planes of a raw frame of `format`: the bytes of each row and the
/// count of rows.
fn planes(format: &str, size: Size) -> Option<Vec<(usize, usize)>> {
    let (width, height) = (size.width as usize, size.height as usize);
    let (chroma_width, chroma_height) = (width.div_ceil(2), height.div_ceil(2));
    Some(match format {
        "NV12" => vec![(width, height), (chroma_width * 2, chroma_height)],
        "I420" => vec![(width, height), (chroma_width, chroma_height), (chroma_width, chroma_height)],
        "RGBA" | "RGBx" => vec![(width * 4, height)],
        _ => return None,
    })
}

/// GStreamer's layout of a frame of `format` whose buffer has no video
/// meta: rows of 4-byte-aligned strides, and planes one after another.
fn default_layout(format: &str, size: Size) -> Layout {
    let round_up_2 = |n: usize| n.next_multiple_of(2);
    let round_up_4 = |n: usize| n.next_multiple_of(4);
    let (width, height) = (size.width as usize, size.height as usize);
    match format {
        "NV12" => {
            let stride = round_up_4(width);
            Layout { offsets: [0, stride * round_up_2(height), 0, 0], strides: [stride, stride, 0, 0] }
        }
        "I420" => {
            let luma = round_up_4(width);
            let chroma = round_up_4(round_up_2(width) / 2);
            let u = luma * round_up_2(height);
            let v = u + chroma * (round_up_2(height) / 2);
            Layout { offsets: [0, u, v, 0], strides: [luma, chroma, chroma, 0] }
        }
        _ => Layout { offsets: [0; 4], strides: [width * 4, 0, 0, 0] },
    }
}

/// The video meta's layout of `buffer`, if it has one of `planes` planes.
///
/// # Safety
///
/// `buffer` is a live `GstBuffer`.
unsafe fn meta_layout(api: &Api, buffer: *mut c_void, planes: usize) -> Option<Layout> {
    // SAFETY: as the caller says; the meta is the buffer's.
    let meta = unsafe { (api.buffer_get_video_meta)(buffer).as_ref()? };
    if (meta.n_planes as usize) < planes {
        return None;
    }
    let mut layout = Layout { offsets: meta.offset, strides: [0; 4] };
    for (stride, &meta_stride) in layout.strides.iter_mut().zip(&meta.stride) {
        *stride = usize::try_from(meta_stride).ok()?;
    }
    Some(layout)
}

/// The planes of `buffer`, a raw frame of `format`, copied out with tight
/// rows.
///
/// # Safety
///
/// `buffer` is a live `GstBuffer`.
unsafe fn cpu_pixels(
    api: &Api,
    buffer: *mut c_void,
    format: &str,
    size: Size,
    color: YuvColor,
) -> Result<FramePixels, String> {
    let shapes = planes(format, size).ok_or_else(|| format!("frames of {format}"))?;
    // SAFETY: as the caller says.
    let layout = unsafe { meta_layout(api, buffer, shapes.len()) }.unwrap_or_else(|| default_layout(format, size));
    let mut info = ffi::MapInfo::new();
    // SAFETY: as the caller says; the map is undone below.
    if unsafe { (api.buffer_map)(buffer, &raw mut info, ffi::MAP_READ) } == 0 {
        return Err("a buffer that doesn't map".to_owned());
    }
    // SAFETY: a mapped buffer's bytes stay readable until it is unmapped.
    let data = unsafe { std::slice::from_raw_parts(info.data, info.size) };
    let copied: Result<Vec<CpuPlane>, String> = shapes
        .iter()
        .enumerate()
        .map(|(index, &(row_bytes, rows))| {
            copy_plane(data, layout.offsets[index], layout.strides[index], row_bytes, rows)
        })
        .collect();
    // SAFETY: the map above.
    unsafe { (api.buffer_unmap)(buffer, &raw mut info) };
    let mut planes = copied?.into_iter();
    let mut next = || planes.next().ok_or_else(|| "a missing plane".to_owned());
    Ok(match format {
        "NV12" => FramePixels::Nv12 { y: next()?, uv: next()?, color },
        "I420" => FramePixels::I420 { y: next()?, u: next()?, v: next()?, color },
        "RGBx" => {
            let mut plane = next()?;
            for pixel in plane.data.as_chunks_mut::<4>().0 {
                pixel[3] = 255;
            }
            FramePixels::Rgba(plane)
        }
        _ => FramePixels::Rgba(next()?),
    })
}

/// `rows` rows of `row_bytes`, `stride` apart from `offset` in `data`.
fn copy_plane(data: &[u8], offset: usize, stride: usize, row_bytes: usize, rows: usize) -> Result<CpuPlane, String> {
    let end = offset + stride * rows.saturating_sub(1) + row_bytes;
    if stride < row_bytes || end > data.len() {
        return Err(format!("a buffer of {} bytes, short of its plane at {offset}", data.len()));
    }
    let mut plane = Vec::with_capacity(row_bytes * rows);
    for row in 0..rows {
        let start = offset + row * stride;
        plane.extend_from_slice(&data[start..start + row_bytes]);
    }
    Ok(CpuPlane { data: plane, stride: row_bytes })
}

/// The DRM fourcc and modifier of caps' `drm-format`, such as
/// `NV12:0x0200000018601b04`, or `NV12` for a linear one.
fn parse_drm_format(text: &str) -> Option<(u32, u64)> {
    let (fourcc, modifier) = text.split_once(':').unwrap_or((text, "0x0"));
    let fourcc: [u8; 4] = fourcc.as_bytes().try_into().ok()?;
    let modifier = u64::from_str_radix(modifier.strip_prefix("0x")?, 16).ok()?;
    Some((u32::from_le_bytes(fourcc), modifier))
}

/// The dmabuf of `buffer`, its descriptors duplicated so that the frame
/// owns them.
///
/// # Safety
///
/// `buffer` is a live `GstBuffer`.
unsafe fn dma_buf(api: &Api, buffer: *mut c_void, drm_format: &str, color: YuvColor) -> Result<GpuFrame, String> {
    let (fourcc, modifier) =
        parse_drm_format(drm_format).ok_or_else(|| format!("a dmabuf of drm-format \"{drm_format}\""))?;
    // SAFETY: as the caller says; the meta is the buffer's.
    let meta = unsafe { (api.buffer_get_video_meta)(buffer).as_ref() }
        .ok_or_else(|| "a dmabuf without a video meta".to_owned())?;
    let count = (meta.n_planes as usize).min(4);
    let mut planes = Vec::with_capacity(count);
    for index in 0..count {
        let (mut memory_index, mut length, mut skip) = (0, 0, 0);
        // SAFETY: as the caller says; the memory is the buffer's.
        let memory = unsafe {
            if (api.buffer_find_memory)(
                buffer,
                meta.offset[index],
                1,
                &raw mut memory_index,
                &raw mut length,
                &raw mut skip,
            ) == 0
            {
                return Err(format!("a dmabuf without memory for plane {index}"));
            }
            (api.buffer_peek_memory)(buffer, memory_index)
        };
        // SAFETY: the buffer's memory, live while the buffer is.
        let (fd, memory_offset) = unsafe {
            if memory.is_null() || (api.is_dmabuf_memory)(memory) == 0 {
                return Err("GPU memory that isn't a dmabuf".to_owned());
            }
            let mut offset = 0;
            (api.memory_get_sizes)(memory, &raw mut offset, std::ptr::null_mut());
            ((api.dmabuf_memory_get_fd)(memory), offset)
        };
        if fd < 0 {
            return Err("a dmabuf without a descriptor".to_owned());
        }
        // SAFETY: the memory's descriptor, open while the buffer lives,
        // which is through this call.
        let fd = unsafe { BorrowedFd::borrow_raw(fd) }
            .try_clone_to_owned()
            .map_err(|error| format!("a dmabuf that doesn't duplicate: {error}"))?;
        let stride = u32::try_from(meta.stride[index]).map_err(|_| "a dmabuf of negative stride".to_owned())?;
        planes.push(DmaBufPlane { fd, offset: (memory_offset + skip) as u32, stride });
    }
    if planes.is_empty() {
        return Err("a dmabuf without planes".to_owned());
    }
    Ok(GpuFrame::DmaBuf { fourcc, modifier, planes, color })
}

/// Whether caps' first structure is video, for a decoder's pad.
///
/// # Safety
///
/// `caps` is live caps.
pub(super) unsafe fn is_video_caps(api: &Api, caps: *const c_void) -> bool {
    // SAFETY: as the caller says.
    unsafe {
        let structure = (api.caps_get_structure)(caps, 0);
        !structure.is_null() && CStr::from_ptr((api.structure_get_name)(structure)).to_bytes().starts_with(b"video/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drm_formats_name_their_fourcc_and_modifier() {
        let nv12 = u32::from_le_bytes(*b"NV12");
        assert_eq!(Some((nv12, 0x0200_0000_1860_1b04)), parse_drm_format("NV12:0x0200000018601b04"));
        assert_eq!(Some((nv12, 0)), parse_drm_format("NV12"));
        assert_eq!(None, parse_drm_format("NV1"));
    }

    #[test]
    fn default_layouts_align_rows_to_four_bytes() {
        let size = Size::new(63, 47);
        let i420 = default_layout("I420", size);
        assert_eq!([64, 32, 32], i420.strides[..3]);
        assert_eq!([0, 64 * 48, 64 * 48 + 32 * 24], i420.offsets[..3]);
        let nv12 = default_layout("NV12", size);
        assert_eq!([64, 64], nv12.strides[..2]);
        assert_eq!(64 * 48, nv12.offsets[1]);
    }

    #[test]
    fn planes_copy_out_without_padding() {
        let data: Vec<u8> = (0..12).collect();
        let plane = copy_plane(&data, 2, 4, 3, 2).unwrap();
        assert_eq!(CpuPlane { data: vec![2, 3, 4, 6, 7, 8], stride: 3 }, plane);
        assert!(copy_plane(&data, 8, 4, 3, 2).is_err());
    }
}
