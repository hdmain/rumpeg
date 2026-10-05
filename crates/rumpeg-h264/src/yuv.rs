//! Planar YUV 4:2:0 frame buffer.

/// 8-bit planar YUV 4:2:0 picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Yuv420Planar {
    /// Luma width in pixels (multiple of 2; encoder pads to 16).
    pub width: u32,
    /// Luma height in pixels.
    pub height: u32,
    /// Y plane (`width * height`).
    pub y: Vec<u8>,
    /// U plane (`(width/2) * (height/2)`).
    pub u: Vec<u8>,
    /// V plane.
    pub v: Vec<u8>,
}

impl Yuv420Planar {
    /// Allocate a mid-gray frame (Y=128, UV=128).
    pub fn gray(width: u32, height: u32) -> Self {
        let y_size = (width * height) as usize;
        let c_size = ((width / 2) * (height / 2)) as usize;
        Self {
            width,
            height,
            y: vec![128; y_size],
            u: vec![128; c_size],
            v: vec![128; c_size],
        }
    }

    /// Allocate zeroed planes.
    pub fn zeroed(width: u32, height: u32) -> Self {
        let y_size = (width * height) as usize;
        let c_size = ((width / 2) * (height / 2)) as usize;
        Self {
            width,
            height,
            y: vec![0; y_size],
            u: vec![0; c_size],
            v: vec![0; c_size],
        }
    }

    /// Round width/height up to macroblock (16) and chroma (2) alignment.
    pub fn aligned_dims(width: u32, height: u32) -> (u32, u32) {
        let w = width.div_ceil(16) * 16;
        let h = height.div_ceil(16) * 16;
        (w.max(16), h.max(16))
    }

    /// Copy from an unaligned frame into a 16×16-padded buffer (edge extend).
    pub fn pad_to_mbs(src: &Self) -> Self {
        let (aw, ah) = Self::aligned_dims(src.width, src.height);
        if aw == src.width && ah == src.height {
            return src.clone();
        }
        let mut dst = Self::zeroed(aw, ah);
        for row in 0..src.height {
            let sy = (row * src.width) as usize;
            let dy = (row * aw) as usize;
            dst.y[dy..dy + src.width as usize]
                .copy_from_slice(&src.y[sy..sy + src.width as usize]);
            // extend right
            let edge = src.y[sy + src.width as usize - 1];
            for x in src.width..aw {
                dst.y[dy + x as usize] = edge;
            }
        }
        // extend bottom
        for row in src.height..ah {
            let prev = ((src.height - 1) * aw) as usize;
            let dy = (row * aw) as usize;
            let len = aw as usize;
            dst.y.copy_within(prev..prev + len, dy);
        }
        let cw = src.width / 2;
        let ch = src.height / 2;
        let acw = aw / 2;
        let ach = ah / 2;
        for row in 0..ch {
            let su = (row * cw) as usize;
            let du = (row * acw) as usize;
            dst.u[du..du + cw as usize].copy_from_slice(&src.u[su..su + cw as usize]);
            dst.v[du..du + cw as usize].copy_from_slice(&src.v[su..su + cw as usize]);
            let eu = src.u[su + cw as usize - 1];
            let ev = src.v[su + cw as usize - 1];
            for x in cw..acw {
                dst.u[du + x as usize] = eu;
                dst.v[du + x as usize] = ev;
            }
        }
        for row in ch..ach {
            let prev = ((ch - 1) * acw) as usize;
            let du = (row * acw) as usize;
            dst.u.copy_within(prev..prev + acw as usize, du);
            dst.v.copy_within(prev..prev + acw as usize, du);
        }
        dst
    }

    /// Crop to exact `width`×`height` (top-left).
    pub fn crop(&self, width: u32, height: u32) -> Self {
        assert!(width <= self.width && height <= self.height);
        let mut out = Self::zeroed(width, height);
        for row in 0..height {
            let s = (row * self.width) as usize;
            let d = (row * width) as usize;
            out.y[d..d + width as usize].copy_from_slice(&self.y[s..s + width as usize]);
        }
        let cw = width / 2;
        let ch = height / 2;
        let scw = self.width / 2;
        for row in 0..ch {
            let s = (row * scw) as usize;
            let d = (row * cw) as usize;
            out.u[d..d + cw as usize].copy_from_slice(&self.u[s..s + cw as usize]);
            out.v[d..d + cw as usize].copy_from_slice(&self.v[s..s + cw as usize]);
        }
        out
    }
}
