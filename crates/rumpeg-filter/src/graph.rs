//! Filter graph construction and execution.

use crate::filters::{
    AFormatFilter, AresampleFilter, CropFilter, EqFilter, FormatFilter, FpsFilter, FramestepFilter,
    HFlipFilter, HueFilter, OverlayFilter, PadFilter, ScaleFilter, TransposeFilter, VFlipFilter,
    VolumeFilter,
};
use rumpeg_util::{Error, Frame, Result};

/// A single filter node.
pub trait Filter: Send {
    /// Filter name.
    fn name(&self) -> &str;
    /// Process one frame into zero or more output frames (usually one).
    fn filter(&mut self, frame: Frame) -> Result<Vec<Frame>>;
    /// Optional: set source frame rate for rate-conversion filters.
    fn set_source_fps(&mut self, _source_fps: u32) {}
    /// Optional: report a target output frame rate (fps filter).
    fn target_fps(&self) -> Option<u32> {
        None
    }
}

/// Ordered chain of filters.
pub struct Graph {
    filters: Vec<Box<dyn Filter>>,
}

impl Graph {
    /// Empty graph (identity).
    pub fn new() -> Self {
        Self {
            filters: Vec::new(),
        }
    }

    /// Parse a comma-separated filterchain.
    pub fn parse(spec: &str) -> Result<Self> {
        let mut graph = Self::new();
        let spec = spec.trim();
        if spec.is_empty() {
            return Ok(graph);
        }
        for part in spec.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            graph.filters.push(parse_one(part)?);
        }
        Ok(graph)
    }

    /// Append a filter.
    pub fn push(&mut self, filter: Box<dyn Filter>) {
        self.filters.push(filter);
    }

    /// Run the graph on a single frame (requires ≥1 output).
    pub fn run(&mut self, frame: Frame) -> Result<Frame> {
        self.run_all(frame)?
            .into_iter()
            .next()
            .ok_or_else(|| Error::invalid_data("filter graph produced no frames"))
    }

    /// Run the graph; may return zero frames (e.g. fps drop / framestep).
    pub fn run_all(&mut self, frame: Frame) -> Result<Vec<Frame>> {
        let mut frames = vec![frame];
        for filter in &mut self.filters {
            let mut next = Vec::new();
            for frame in frames {
                next.extend(filter.filter(frame)?);
            }
            frames = next;
        }
        Ok(frames)
    }

    /// Set source fps on any `fps` filters in the graph.
    pub fn set_fps_source(&mut self, source_fps: u32) {
        for f in &mut self.filters {
            f.set_source_fps(source_fps);
        }
    }

    /// Target fps from the last `fps` filter in the chain, if any.
    pub fn target_fps(&self) -> Option<u32> {
        self.filters.iter().rev().find_map(|f| f.target_fps())
    }
}

impl Default for Graph {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_one(spec: &str) -> Result<Box<dyn Filter>> {
    let (name, args) = match spec.split_once('=') {
        Some((n, a)) => (n, a),
        None => (spec, ""),
    };
    Ok(match name {
        "volume" => Box::new(VolumeFilter::parse(args)?),
        "aformat" => Box::new(AFormatFilter::parse(args)?),
        "format" => Box::new(FormatFilter::parse(args)?),
        "scale" => Box::new(ScaleFilter::parse(args)?),
        "hflip" => Box::new(HFlipFilter::parse(args)?),
        "vflip" => Box::new(VFlipFilter::parse(args)?),
        "crop" => Box::new(CropFilter::parse(args)?),
        "pad" => Box::new(PadFilter::parse(args)?),
        "transpose" => Box::new(TransposeFilter::parse(args)?),
        "fps" => Box::new(FpsFilter::parse(args)?),
        "framestep" => Box::new(FramestepFilter::parse(args)?),
        "eq" => Box::new(EqFilter::parse(args)?),
        "hue" => Box::new(HueFilter::parse(args)?),
        "aresample" => Box::new(AresampleFilter::parse(args)?),
        "overlay" => Box::new(OverlayFilter::parse(args)?),
        "null" | "anull" => Box::new(NullFilter),
        other => {
            return Err(Error::not_found(format!("filter '{other}'")));
        }
    })
}

struct NullFilter;

impl Filter for NullFilter {
    fn name(&self) -> &str {
        "null"
    }
    fn filter(&mut self, frame: Frame) -> Result<Vec<Frame>> {
        Ok(vec![frame])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rumpeg_util::{Frame, PixelFormat, VideoFrame};

    fn rgb2(w: u32, h: u32, pixels: &[(u8, u8, u8)]) -> VideoFrame {
        let mut f = VideoFrame::alloc(PixelFormat::Rgb24, w, h);
        let plane = f.plane_mut(0).unwrap();
        for (i, (r, g, b)) in pixels.iter().enumerate() {
            let o = i * 3;
            plane[o] = *r;
            plane[o + 1] = *g;
            plane[o + 2] = *b;
        }
        f
    }

    #[test]
    fn graph_parses_crop_and_crops_frame() {
        let mut graph = Graph::parse("crop=1:1:1:0").unwrap();
        let src = rgb2(2, 1, &[(10, 20, 30), (40, 50, 60)]);
        let out = graph.run(Frame::Video(src)).unwrap();
        let Frame::Video(v) = out else {
            panic!("expected video");
        };
        assert_eq!(v.width, 1);
        assert_eq!(v.height, 1);
        let p = v.plane(0).unwrap();
        assert_eq!(&p[..3], &[40, 50, 60]);
    }

    #[test]
    fn graph_parses_vflip_and_flips_rows() {
        let mut graph = Graph::parse("vflip").unwrap();
        let src = rgb2(1, 2, &[(1, 2, 3), (4, 5, 6)]);
        let out = graph.run(Frame::Video(src)).unwrap();
        let Frame::Video(v) = out else {
            panic!("expected video");
        };
        let p = v.plane(0).unwrap();
        assert_eq!(&p[..3], &[4, 5, 6]);
        assert_eq!(&p[3..6], &[1, 2, 3]);
    }
}
