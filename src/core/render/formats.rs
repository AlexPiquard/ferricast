use anyhow::Result;
use ges::Pipeline;
use ges::gst_pbutils::{EncodingContainerProfile, EncodingVideoProfile};
use ges::prelude::*;
use gst::Element;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use crate::core::render::pipeline::OnProgressFn;
use crate::core::utils;

pub static FORMATS: LazyLock<[Box<dyn PipelineSpec>; 2]> =
    LazyLock::new(|| [Box::new(Mp4), Box::new(Gif)]);

pub fn formats() -> Vec<&'static str> {
    FORMATS.iter().map(|f| f.name()).collect()
}

pub trait PipelineSpec: Send + Sync {
    fn setup(&self, pipeline: &Pipeline, output: &Path) -> Result<()>;
    fn finalize(&self, output: &Path, pipeline: &Pipeline, on_progress: OnProgressFn)
    -> Result<()>;
    fn extension(&self) -> &str;
    fn name(&self) -> &str;
    fn pipeline_max_progress(&self) -> f64;
}

struct Mp4;
impl PipelineSpec for Mp4 {
    fn setup(&self, pipeline: &Pipeline, output: &Path) -> Result<()> {
        let container_caps = gst::Caps::builder("video/quicktime")
            .field("variant", "iso")
            .build();

        let restriction_caps = gst::Caps::builder("video/x-raw")
            .field("framerate", gst::Fraction::new(30, 1))
            .build();
        let video_caps = gst::Caps::builder("video/x-h264").build();
        let video_profile = EncodingVideoProfile::builder(&video_caps)
            .restriction(&restriction_caps)
            .build();
        let profile = EncodingContainerProfile::builder(&container_caps)
            .name("mp4-export")
            .description("Export MP4 H264/AAC")
            .add_profile(video_profile)
            .build();

        let uri = format!("file://{}", output.to_string_lossy());
        pipeline.set_render_settings(&uri, &profile)?;
        pipeline.set_mode(ges::PipelineFlags::RENDER)?;

        let mut iter = pipeline.iterate_sinks();
        if let Ok(Some(sink_element)) = iter.next() {
            sink_element.set_property("sync", false);
            sink_element.set_property("async", false);
        }

        Ok(())
    }

    fn finalize(
        &self,
        _output: &Path,
        _pipeline: &Pipeline,
        _on_progress: OnProgressFn,
    ) -> Result<()> {
        Ok(())
    }

    fn extension(&self) -> &str {
        "mp4"
    }

    fn name(&self) -> &str {
        "MP4"
    }

    fn pipeline_max_progress(&self) -> f64 {
        1.0
    }
}

struct GifskiProgressBridge {
    callback: OnProgressFn,
    min_progress: f64,
    total: i32,
    current: i32,
}

impl GifskiProgressBridge {
    pub fn new(total: i32, min_progress: f64, callback: OnProgressFn) -> GifskiProgressBridge {
        GifskiProgressBridge {
            callback,
            min_progress,
            total,
            current: 0,
        }
    }
}

impl gifski::progress::ProgressReporter for GifskiProgressBridge {
    fn increase(&mut self) -> bool {
        self.current += 1;
        if let Some(callback) = &self.callback {
            callback(
                self.min_progress + (self.current as f64 / self.total as f64) * self.min_progress,
            );
        }
        true
    }
}

struct Gif;
impl PipelineSpec for Gif {
    fn setup(&self, pipeline: &Pipeline, output: &Path) -> Result<()> {
        let tmp_path = self.tmp_path(output);

        // not working in render mode
        pipeline.set_mode(ges::PipelineFlags::VIDEO_PREVIEW)?;

        let videoconvert = gst::ElementFactory::make("videoconvert").build()?;
        let videorate = gst::ElementFactory::make("videorate").build()?;
        let pngenc = gst::ElementFactory::make("pngenc").build()?;
        let filesink = gst::ElementFactory::make("multifilesink")
            .name("filesink")
            .property(
                "location",
                format!("{}%06d.png", tmp_path.to_string_lossy()),
            )
            .build()?;

        let caps = gst::Caps::builder("video/x-raw")
            .field("framerate", gst::Fraction::new(30, 1))
            .build();
        let capsfilter = gst::ElementFactory::make("capsfilter")
            .property("caps", &caps) // TODO: configure this ? and for mp4 ?
            .build()?;

        let sink_bin = gst::Bin::new();
        sink_bin.add_many([&videoconvert, &capsfilter, &videorate, &pngenc, &filesink])?;
        gst::Element::link_many([&videoconvert, &capsfilter, &videorate, &pngenc, &filesink])?;

        let pad = videoconvert.static_pad("sink").unwrap();
        let ghost_pad = gst::GhostPad::with_target(&pad)?;
        sink_bin.add_pad(&ghost_pad)?;

        pipeline.set_property("video-sink", &sink_bin);

        Ok(())
    }

    fn finalize(
        &self,
        output: &Path,
        pipeline: &Pipeline,
        on_progress: OnProgressFn,
    ) -> Result<()> {
        let tmp_path = self.tmp_path(output);
        let sink_bin = pipeline.property::<gst::Bin>("video-sink");
        let Some(filesink) = sink_bin.by_name("filesink") else {
            tracing::warn!("failed to retrieve filesink after rendering");
            return Ok(());
        };
        self.run_gifski(
            tmp_path.to_string_lossy().to_string(),
            output,
            pipeline.clone(),
            filesink,
            on_progress,
        )?;
        std::fs::remove_dir_all(tmp_path.parent().unwrap()).ok();
        Ok(())
    }

    fn extension(&self) -> &str {
        "gif"
    }

    fn name(&self) -> &str {
        "Gif"
    }

    fn pipeline_max_progress(&self) -> f64 {
        0.5
    }
}
impl Gif {
    fn run_gifski(
        &self,
        tmp_path: String,
        output_path: &Path,
        pipeline: Pipeline,
        filesink: Element,
        on_progress: OnProgressFn,
    ) -> anyhow::Result<()> {
        let (width, height) = video_dimension_from_pipeline(pipeline);

        // PERF: takes 10 minutes for gif in high quality
        let gifski_settings = gifski::Settings {
            width,
            height,
            quality: 100,
            fast: false,
            repeat: gifski::Repeat::Infinite,
        };
        let (collector, writer) = gifski::new(gifski_settings)?;
        let output_no_ext = output_path
            .with_extension("")
            .to_string_lossy()
            .into_owned();
        std::thread::scope(|t| -> Result<(), anyhow::Error> {
            let output_path = format!("{output_no_ext}.gif");
            let files: i32 = filesink.property::<i32>("index");
            let range = Range {
                start: 0,
                end: files as usize,
            };
            let frames_thread = t.spawn(move || {
                for i in range {
                    collector.add_frame_png_file(
                        i,
                        format!("{}{i:06}.png", tmp_path).into(),
                        i as f64 * (1.0 / 30.0),
                    )?;
                }
                drop(collector);
                Ok(())
            });

            let mut progress_bridge =
                GifskiProgressBridge::new(files, 1.0 - self.pipeline_max_progress(), on_progress);

            writer.write(std::fs::File::create(output_path)?, &mut progress_bridge)?;
            frames_thread.join().unwrap()
        })?;
        Ok(())
    }

    fn tmp_path(&self, output: &Path) -> PathBuf {
        let output_name = output.file_stem().unwrap().to_string_lossy().to_string();
        let tmp_dir = utils::tmp_dir(Some(output_name.clone()));
        tmp_dir.join(output_name)
    }
}

fn video_dimension_from_pipeline(pipeline: Pipeline) -> (Option<u32>, Option<u32>) {
    let Some(timeline) = pipeline.timeline() else {
        return (None, None);
    };
    let tracks = timeline.tracks();

    for track in tracks {
        if track.track_type() == ges::TrackType::VIDEO
            && let Some(caps) = track.restriction_caps()
        {
            let structure = caps.structure(0).unwrap();
            if let Ok(width) = structure.get::<i32>("width")
                && let Ok(height) = structure.get::<i32>("height")
            {
                return (Some(width as u32), Some(height as u32));
            }
        }
    }

    (None, None)
}
