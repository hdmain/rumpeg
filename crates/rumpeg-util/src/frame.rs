//! Decoded media frames (`AVFrame` counterpart).

use crate::buffer::Buffer;
use crate::media::{ChannelLayout, PixelFormat, SampleFormat};
use crate::time::Timestamp;

/// Maximum number of data planes in a frame.
pub const MAX_PLANES: usize = 8;

/// A decoded audio or video frame.
#[derive(Clone, Debug)]
pub enum Frame {
    /// Video picture.
    Video(VideoFrame),
    /// Audio PCM buffer.
    Audio(AudioFrame),
}

impl Frame {
    /// Presentation timestamp.
    pub fn pts(&self) -> Timestamp {
        match self {
            Self::Video(v) => v.pts,
            Self::Audio(a) => a.pts,
        }
    }

    /// Set presentation timestamp.
    pub fn set_pts(&mut self, pts: Timestamp) {
        match self {
            Self::Video(v) => v.pts = pts,
            Self::Audio(a) => a.pts = pts,
        }
    }
}

/// Planar / packed video frame.
#[derive(Clone, Debug)]
pub struct VideoFrame {
    /// Pixel format.
    pub format: PixelFormat,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Plane data (shared buffers).
    pub planes: Vec<Buffer>,
    /// Stride (bytes per row) for each plane.
    pub strides: Vec<usize>,
    /// Presentation timestamp.
    pub pts: Timestamp,
    /// Picture type / keyframe hint.
    pub key_frame: bool,
}

impl VideoFrame {
    /// Allocate a zeroed video frame for `format` at `width`×`height`.
    pub fn alloc(format: PixelFormat, width: u32, height: u32) -> Self {
        let layout = plane_layout(format, width, height);
        let planes = layout
            .iter()
            .map(|p| Buffer::zeroed(p.size))
            .collect::<Vec<_>>();
        let strides = layout.iter().map(|p| p.stride).collect();
        Self {
            format,
            width,
            height,
            planes,
            strides,
            pts: Timestamp::NONE,
            key_frame: false,
        }
    }

    /// Borrow plane `index` as bytes.
    pub fn plane(&self, index: usize) -> Option<&[u8]> {
        self.planes.get(index).map(|b| b.as_slice())
    }

    /// Mutable access to plane `index` (COW).
    pub fn plane_mut(&mut self, index: usize) -> Option<&mut [u8]> {
        self.planes.get_mut(index).map(|b| b.make_mut())
    }

    /// Total allocated bytes across planes.
    pub fn total_bytes(&self) -> usize {
        self.planes.iter().map(|p| p.len()).sum()
    }
}

/// Interleaved or planar audio frame.
#[derive(Clone, Debug)]
pub struct AudioFrame {
    /// Sample format.
    pub format: SampleFormat,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Channel layout.
    pub layout: ChannelLayout,
    /// Number of samples per channel.
    pub samples: u32,
    /// Sample data. Interleaved formats use a single plane; planar use one per channel.
    pub planes: Vec<Buffer>,
    /// Presentation timestamp.
    pub pts: Timestamp,
}

impl AudioFrame {
    /// Allocate a zeroed interleaved audio frame.
    pub fn alloc_interleaved(
        format: SampleFormat,
        sample_rate: u32,
        layout: ChannelLayout,
        samples: u32,
    ) -> Self {
        assert!(
            !format.is_planar(),
            "use alloc_planar for planar sample formats"
        );
        let bytes = format.bytes_per_sample() * layout.channels as usize * samples as usize;
        Self {
            format,
            sample_rate,
            layout,
            samples,
            planes: vec![Buffer::zeroed(bytes)],
            pts: Timestamp::NONE,
        }
    }

    /// Allocate a zeroed planar audio frame.
    pub fn alloc_planar(
        format: SampleFormat,
        sample_rate: u32,
        layout: ChannelLayout,
        samples: u32,
    ) -> Self {
        assert!(format.is_planar(), "format must be planar");
        let plane_bytes = format.bytes_per_sample() * samples as usize;
        let planes = (0..layout.channels)
            .map(|_| Buffer::zeroed(plane_bytes))
            .collect();
        Self {
            format,
            sample_rate,
            layout,
            samples,
            planes,
            pts: Timestamp::NONE,
        }
    }

    /// Borrow interleaved PCM bytes (single plane).
    pub fn data(&self) -> &[u8] {
        self.planes.first().map(|p| p.as_slice()).unwrap_or(&[])
    }

    /// Mutable interleaved PCM bytes.
    pub fn data_mut(&mut self) -> &mut [u8] {
        if self.planes.is_empty() {
            self.planes.push(Buffer::from_vec(Vec::new()));
        }
        self.planes[0].make_mut()
    }

    /// Byte length of interleaved payload.
    pub fn data_len(&self) -> usize {
        self.data().len()
    }
}

#[derive(Clone, Copy)]
struct PlaneInfo {
    stride: usize,
    size: usize,
}

fn plane_layout(format: PixelFormat, width: u32, height: u32) -> Vec<PlaneInfo> {
    let w = width as usize;
    let h = height as usize;
    match format {
        PixelFormat::Rgb24 | PixelFormat::Bgr24 => {
            let stride = w * 3;
            vec![PlaneInfo {
                stride,
                size: stride * h,
            }]
        }
        PixelFormat::Rgba | PixelFormat::Bgra => {
            let stride = w * 4;
            vec![PlaneInfo {
                stride,
                size: stride * h,
            }]
        }
        PixelFormat::Gray8 => {
            vec![PlaneInfo {
                stride: w,
                size: w * h,
            }]
        }
        PixelFormat::Yuv420p => {
            let cw = w.div_ceil(2);
            let ch = h.div_ceil(2);
            vec![
                PlaneInfo {
                    stride: w,
                    size: w * h,
                },
                PlaneInfo {
                    stride: cw,
                    size: cw * ch,
                },
                PlaneInfo {
                    stride: cw,
                    size: cw * ch,
                },
            ]
        }
        PixelFormat::Yuv422p => {
            let cw = w.div_ceil(2);
            vec![
                PlaneInfo {
                    stride: w,
                    size: w * h,
                },
                PlaneInfo {
                    stride: cw,
                    size: cw * h,
                },
                PlaneInfo {
                    stride: cw,
                    size: cw * h,
                },
            ]
        }
        PixelFormat::Yuv444p => vec![
            PlaneInfo {
                stride: w,
                size: w * h,
            },
            PlaneInfo {
                stride: w,
                size: w * h,
            },
            PlaneInfo {
                stride: w,
                size: w * h,
            },
        ],
        PixelFormat::Nv12 => {
            let cw = w.div_ceil(2) * 2;
            let ch = h.div_ceil(2);
            vec![
                PlaneInfo {
                    stride: w,
                    size: w * h,
                },
                PlaneInfo {
                    stride: cw,
                    size: cw * ch,
                },
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yuv420_alloc_size() {
        let f = VideoFrame::alloc(PixelFormat::Yuv420p, 320, 240);
        assert_eq!(f.planes.len(), 3);
        assert_eq!(f.planes[0].len(), 320 * 240);
        assert_eq!(f.planes[1].len(), 160 * 120);
        assert_eq!(f.total_bytes(), 320 * 240 + 2 * 160 * 120);
    }
}
