//! Filter graph construction and execution.

use crate::filters::{AFormatFilter, FormatFilter, ScaleFilter, VolumeFilter};
use rumpeg_util::{Error, Frame, Result};

/// A single filter node.
pub trait Filter: Send {
    /// Filter name.
    fn name(&self) -> &str;
    /// Process one frame into zero or more output frames (usually one).
    fn filter(&mut self, frame: Frame) -> Result<Vec<Frame>>;
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

    /// Run the graph on a single frame.
    pub fn run(&mut self, frame: Frame) -> Result<Frame> {
        let mut frames = vec![frame];
        for filter in &mut self.filters {
            let mut next = Vec::new();
            for frame in frames {
                next.extend(filter.filter(frame)?);
            }
            frames = next;
        }
        frames
            .into_iter()
            .next()
            .ok_or_else(|| Error::invalid_data("filter graph produced no frames"))
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
