use ges::Pipeline;
use ges::prelude::*;
use std::path::PathBuf;
use std::sync::Arc;

use crate::core::render::RenderSettings;
use crate::core::video;

pub fn render<F>(
    settings: RenderSettings,
    input: PathBuf,
    output: PathBuf,
    zoom_effects: Vec<video::ZoomEffect>,
    on_progress: F,
) where
    F: Fn(f64) + Send + Sync + 'static,
{
    let mut render = Render::new(settings, input, output, zoom_effects);
    render.on_progress(on_progress);
    if let Err(e) = render.start() {
        tracing::warn!("failed to render: {:?}", e);
    }
}

pub type OnProgressFn = Option<Arc<dyn Fn(f64) + Send + Sync + 'static>>;

struct Render {
    pipeline: Option<Pipeline>,
    settings: RenderSettings,
    input: PathBuf,
    output: PathBuf,
    zoom_effects: Vec<video::ZoomEffect>,
    on_progress: OnProgressFn,
}

impl Render {
    pub fn new(
        settings: RenderSettings,
        input: PathBuf,
        output: PathBuf,
        zoom_effects: Vec<video::ZoomEffect>,
    ) -> Self {
        let mut s = Self {
            pipeline: None,
            settings,
            input,
            output,
            zoom_effects,
            on_progress: None,
        };
        if let Err(e) = s.setup() {
            tracing::warn!("failed to setup render pipeline: {:?}", e);
        }
        s
    }

    fn setup(&mut self) -> anyhow::Result<()> {
        let mut timeline = video::Video::try_new(self.input.clone(), None::<fn(bool)>)?;
        timeline.setup_cursor()?;
        timeline.apply_zoom_effects(&self.zoom_effects)?;
        timeline.redraw_cursor();
        self.pipeline = Some(timeline.pipeline());
        self.settings
            .format()
            .setup(self.pipeline.as_ref().unwrap(), &self.output)?;
        Ok(())
    }

    pub fn on_progress<F>(&mut self, on_progress: F)
    where
        F: Fn(f64) + Send + Sync + 'static,
    {
        self.on_progress = Some(Arc::new(on_progress));
    }

    pub fn start(&self) -> anyhow::Result<()> {
        let pipeline = self.pipeline.as_ref().unwrap();
        let start_time = self.settings.start_sec();
        let end_time = self.settings.end_sec();

        if start_time > 0.0 || end_time.is_some() {
            pipeline.set_state(gst::State::Paused)?;
            let _ = pipeline.state(gst::ClockTime::NONE);

            let (stop_type, stop_clocktime) = if let Some(end) = end_time {
                (
                    gst::SeekType::Set,
                    Some(gst::ClockTime::from_seconds_f64(end)),
                )
            } else {
                (gst::SeekType::None, gst::ClockTime::NONE)
            };

            let seek_event = gst::event::Seek::new(
                1.0,
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::SeekType::Set,
                gst::ClockTime::from_seconds_f64(start_time),
                stop_type,
                stop_clocktime,
            );

            if !pipeline.send_event(seek_event) {
                tracing::warn!("failed to send seek event");
            }
        }

        pipeline.set_state(gst::State::Playing)?;

        let callback = self.on_progress.as_ref().map(|c| c.clone());
        let pipeline_max_progress = self.settings.format().pipeline_max_progress();

        let bus = pipeline.bus().unwrap();
        'main_loop: loop {
            while let Some(msg) = bus.pop() {
                match msg.view() {
                    gst::MessageView::Eos(..) => {
                        if let Some(callback) = callback.as_ref() {
                            callback(pipeline_max_progress);
                        }
                        break 'main_loop;
                    }
                    gst::MessageView::Error(err) => {
                        let error = err.error();
                        let debug = err.debug();
                        println!("GStreamer error: {} ({:?})", error, debug.unwrap());
                        break 'main_loop;
                    }
                    _ => (),
                }
            }

            if let Some(callback) = callback.as_ref()
                && let (Some(pos), Some(dur)) = (
                    pipeline.query_position::<gst::ClockTime>(),
                    pipeline.query_duration::<gst::ClockTime>(),
                )
            {
                let progress =
                    pos.nseconds() as f64 / dur.nseconds() as f64 * pipeline_max_progress;
                if pos < dur {
                    callback(progress);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(33));
        }

        pipeline.set_state(gst::State::Null)?;

        self.settings
            .format()
            .finalize(&self.output, pipeline, callback)?;

        Ok(())
    }
}
