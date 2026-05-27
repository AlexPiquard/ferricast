use crate::core::render::formats::{self, PipelineSpec};

#[derive(Default, Debug, Clone)]
pub struct RenderSettings {
    format_position: u32,
    start_sec: f64,
    end_sec: Option<f64>,
}

impl RenderSettings {
    pub fn new(format_position: u32, start_sec: f64, end_sec: Option<f64>) -> Self {
        Self {
            format_position,
            start_sec,
            end_sec,
        }
    }

    pub fn format(&self) -> &dyn PipelineSpec {
        let id = self.format_position() as usize;
        if formats::FORMATS.len() > id + 1 {
            return &*formats::FORMATS[0];
        }
        &*formats::FORMATS[self.format_position() as usize]
    }

    pub fn format_position(&self) -> u32 {
        self.format_position
    }

    pub fn start_sec(&self) -> f64 {
        self.start_sec
    }

    pub fn end_sec(&self) -> Option<f64> {
        self.end_sec
    }
}
