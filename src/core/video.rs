use std::collections::HashMap;
use std::collections::HashSet;
use std::ops::AddAssign;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use derivative::Derivative;
use ges::Layer;
use ges::prelude::*;
use gst_controller::prelude::*;
use std::fs::File;
use std::io::Read;

use crate::core::cursor::CursorEntry;
use crate::core::cursor::CursorFile;
use crate::core::rdp;
use crate::core::utils;

pub const ZOOM_ANIMATION_NSEC: u64 = 1_000_000_000;
pub const RESIZE_HANDLE_SIZE: f64 = 10.0;

#[derive(Debug, Clone)]
pub struct ZoomEffect {
    pub factor: f64,
    pub start_nsec: u64,
    pub end_nsec: u64,
    pub pos_x: f64,
    pub pos_y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResizeHandle {
    Left,
    Right,
    None,
}

impl ZoomEffect {
    pub fn timeline_bounds(&self, timeline_width: f64, timeline_duration_ns: u64) -> (f64, f64) {
        let time_scale = if timeline_duration_ns > 0 {
            timeline_width / timeline_duration_ns as f64
        } else {
            0.0
        };
        self.timeline_bounds_at_scale(time_scale, timeline_duration_ns)
    }
    pub fn timeline_bounds_at_scale(
        &self,
        time_scale: f64,
        timeline_duration_nsec: u64,
    ) -> (f64, f64) {
        let x = self.start_anim_nsec() as f64 * time_scale;
        let duration = (self.end_anim_nsec(timeline_duration_nsec) - self.start_anim_nsec()) as f64;
        let width = duration * time_scale;

        (x, width)
    }
    pub fn timeline_contains(
        &self,
        target_x: f64,
        timeline_width: f64,
        timeline_duration_ns: u64,
    ) -> bool {
        let (x, width) = self.timeline_bounds(timeline_width, timeline_duration_ns);
        target_x >= x && target_x <= (x + width)
    }

    pub fn start_anim_nsec(&self) -> u64 {
        if self.start_nsec <= ZOOM_ANIMATION_NSEC {
            return 0;
        }
        self.start_nsec - ZOOM_ANIMATION_NSEC
    }

    pub fn end_anim_nsec(&self, video_duration_nsec: u64) -> u64 {
        if self.end_nsec + ZOOM_ANIMATION_NSEC > video_duration_nsec {
            return video_duration_nsec;
        }

        self.end_nsec + ZOOM_ANIMATION_NSEC
    }

    pub fn clocktimes(
        &self,
        video_duration: u64,
    ) -> (
        gst::ClockTime,
        gst::ClockTime,
        gst::ClockTime,
        gst::ClockTime,
    ) {
        let start_anim_ns = gst::ClockTime::from_nseconds(self.start_anim_nsec());
        let start_ns = gst::ClockTime::from_nseconds(self.start_nsec);
        let end_ns = gst::ClockTime::from_nseconds(self.end_nsec);
        let end_anim_ns = gst::ClockTime::from_nseconds(self.end_anim_nsec(video_duration));
        (start_anim_ns, start_ns, end_ns, end_anim_ns)
    }

    pub fn timeline_resize_handle_at(
        &self,
        target_x: f64,
        timeline_width: f64,
        timeline_duration_ns: u64,
    ) -> ResizeHandle {
        let (x, width) = self.timeline_bounds(timeline_width, timeline_duration_ns);

        if target_x >= x && target_x <= x + RESIZE_HANDLE_SIZE {
            ResizeHandle::Left
        } else if target_x >= x + width - RESIZE_HANDLE_SIZE && target_x <= x + width {
            ResizeHandle::Right
        } else {
            ResizeHandle::None
        }
    }
}

type ZoomControlSourcesArray = [(
    &'static str,
    Option<gst_controller::InterpolationControlSource>,
); 4];

#[derive(Default, Clone, Debug)]
struct CursorControlSources {
    posx: gst_controller::InterpolationControlSource,
    posy: gst_controller::InterpolationControlSource,
    alpha: gst_controller::InterpolationControlSource,
}

type CursorControlSourcesTypes = HashMap<u64, CursorControlSources>;

pub type OnCursorToggle = Option<Rc<dyn Fn(bool) + 'static>>;

#[derive(Derivative)]
#[derivative(Default, Clone, Debug)]
pub struct Video {
    pipeline: ges::Pipeline,
    recording_file: PathBuf,
    video_uriclip: Option<ges::UriClip>,
    video_width: u32,
    video_height: u32,
    video_framerate: Option<gst::Fraction>,
    video_duration: Option<gst::ClockTime>,
    video_layer: Layer,
    zoom_cs: ZoomControlSourcesArray,
    zoom_effects: Vec<ZoomEffect>,
    cursor_enabled: bool,
    cursor_smoothing: f64,
    cursor_show: bool, // TODO: apply these settings to render
    cursor_cs: CursorControlSourcesTypes,
    cursor_layers: HashMap<u64, Layer>,
    cursor_used_entries: Vec<usize>,
    cursor_entries: Vec<CursorEntry>,

    #[derivative(Debug = "ignore")]
    cursor_on_toggle: OnCursorToggle,
}

impl Video {
    pub fn try_new<F>(recording_file: PathBuf, on_cursor_toggle: Option<F>) -> Result<Self>
    where
        F: Fn(bool) + 'static,
    {
        let timeline = ges::Timeline::new();

        let video_track = ges::VideoTrack::new();
        timeline
            .add_track(&video_track)
            .expect("failed to add video track");

        let pipeline = ges::Pipeline::new();
        pipeline
            .set_timeline(&timeline)
            .expect("failed to assign timeline to GES pipeline");

        pipeline.set_property(
            "video-filter",
            gst::ElementFactory::make("videoconvert").build().unwrap(),
        );

        let Ok(recording_file) = recording_file.canonicalize() else {
            bail!("unknown file {:?}", recording_file.to_str().unwrap())
        };

        let video_uri = format!("file://{}", recording_file.display());
        let video_asset = ges::UriClipAsset::request_sync(&video_uri)?;
        let video_info = video_asset.info().video_streams()[0].clone();
        let video_width = video_info.width();
        let video_height = video_info.height();
        let video_framerate = video_info.framerate();
        let video_duration = video_asset.duration();
        let caps = gst::Caps::builder("video/x-raw")
            .field("width", video_width as i32)
            .field("height", video_height as i32)
            .field("format", "I420")
            .field("colorimetry", "bt709")
            .build();
        video_track.set_restriction_caps(&caps);

        let video_layer = timeline.append_layer();
        video_layer.set_priority(100);
        let video_clip = video_layer
            .add_asset(
                &video_asset,
                gst::ClockTime::ZERO,
                gst::ClockTime::ZERO,
                video_duration,
                ges::TrackType::VIDEO,
            )
            .expect("failed to add video asset");

        let video_uriclip = video_clip.clone().downcast::<ges::UriClip>().ok();

        let videobox_effect =
            ges::Effect::new("videobox").expect("failed to create videobox effect");
        video_clip
            .add(&videobox_effect)
            .expect("failed to add videobox effect to clip");

        let zoom_cs = Self::setup_zoom(&videobox_effect, video_duration)?;
        video_clip.set_child_property("fwidth", video_width as f32)?;
        video_clip.set_child_property("fheight", video_height as f32)?;

        let cursor_smoothing = 50.0;

        Ok(Self {
            pipeline,
            recording_file,
            video_uriclip,
            video_width,
            video_height,
            video_framerate: Some(video_framerate),
            video_duration,
            video_layer,
            zoom_cs,
            zoom_effects: Vec::new(),
            cursor_enabled: true,
            cursor_on_toggle: on_cursor_toggle.map(|f| Rc::new(f) as Rc<dyn Fn(bool)>),
            cursor_smoothing,
            cursor_show: true,
            cursor_cs: CursorControlSourcesTypes::new(),
            cursor_layers: HashMap::new(),
            cursor_entries: Vec::new(),
            cursor_used_entries: Vec::new(),
        })
    }

    pub fn setup_cursor(&mut self) -> Result<()> {
        let real_path = utils::resolve_portal_path(&self.recording_file);
        let curs_path = real_path.with_extension("curs");
        let curs_path_str = curs_path.to_str().unwrap();
        if !curs_path.exists() {
            self.set_cursor_enabled(false)?;
            bail!(
                "curs file not found at {}, cursor features are disabled",
                curs_path_str
            );
        }

        let (cursor_entries, cursor_type_entries) = read_cursor_entries(curs_path_str)?;

        self.cursor_entries = cursor_entries;
        self.setup_cursor_entries(cursor_type_entries)?;
        Ok(())
    }

    pub fn timeline(&self) -> ges::Timeline {
        self.pipeline
            .timeline()
            .expect("failed to retrieve timeline")
    }

    pub fn pipeline(&self) -> ges::Pipeline {
        self.pipeline.clone()
    }

    pub fn recording_file(&self) -> PathBuf {
        self.recording_file.clone()
    }

    pub fn set_video_sink(&self, sink: &gst::Bin) {
        self.pipeline.set_property("video-sink", sink);
    }

    pub fn start(&self) {
        if let Err(e) = self.pipeline.set_state(gst::State::Playing) {
            tracing::error!("failed to start pipeline: {:?}", e);
        }
    }

    fn zoom_value_at(&self, prop_name: &str, clocktime: gst::ClockTime) -> Option<f64> {
        let cs = self
            .zoom_cs
            .iter()
            .find_map(|(prop, cs)| (*prop == prop_name).then_some(cs))
            .unwrap()
            .as_ref()
            .unwrap();
        gst::prelude::ControlSourceExt::value(cs, clocktime)
    }

    fn video_uriclip(&self) -> &ges::UriClip {
        self.video_uriclip.as_ref().unwrap()
    }

    pub fn video_track_element(&self) -> ges::TrackElement {
        video_track_element(self.video_uriclip())
    }

    pub fn current_position_nsec(&self) -> Option<u64> {
        self.pipeline
            .query_position::<gst::ClockTime>()
            .map(|t| t.nseconds())
    }

    pub fn set_cursor_smoothing(&mut self, value: f64) {
        self.cursor_smoothing = value;
        self.update_cursor_smoothing();
        self.redraw_cursor();
    }

    pub fn set_cursor_show(&mut self, value: bool) -> Result<()> {
        self.cursor_show = value;
        if value {
            self.video_layer.set_priority(100);
            // PERF:
            self.update_cursor_types();
        } else {
            for (_, cs) in self.cursor_cs.iter() {
                cs.alpha.unset_all();
                cs.alpha.set(gst::ClockTime::ZERO, 0.0);
            }
            if self.cursor_cs.is_empty() {
                self.video_layer.set_priority(0);
            }
        }
        self.timeline().commit();
        Ok(())
    }

    pub fn duration(&self) -> Option<gst::ClockTime> {
        self.video_duration
    }

    pub fn duration_nsec(&self) -> u64 {
        self.video_duration.map_or(0, |d| d.nseconds())
    }

    pub fn cursor_smoothing(&self) -> f64 {
        self.cursor_smoothing
    }

    pub fn cursor_show(&self) -> bool {
        self.cursor_show
    }

    pub fn framerate(&self) -> Option<gst::Fraction> {
        self.video_framerate
    }

    pub fn set_cursor_enabled(&mut self, enabled: bool) -> Result<()> {
        self.set_cursor_show(enabled)?;
        self.cursor_enabled = enabled;
        if let Some(callback) = self.cursor_on_toggle.as_ref() {
            callback(enabled);
        }
        Ok(())
    }

    fn setup_cursor_entries(&mut self, cursor_type_entries: Vec<usize>) -> anyhow::Result<()> {
        if !self.cursor_enabled {
            return Ok(());
        }

        let mut i = 0;
        for cursor_index in cursor_type_entries {
            let cursor = self.cursor_entries[cursor_index];
            self.add_cursor_img_layer(i, cursor.cursor_type_hash.unwrap())?;
            i += 1;
        }

        self.update_cursor_smoothing();
        self.update_cursor_types();
        self.redraw_cursor();

        Ok(())
    }

    fn add_cursor_img_layer(&mut self, index: usize, hash: u64) -> Result<()> {
        let layer = self.timeline().append_layer();
        layer.set_priority(index as u32);

        let img_uri = format!(
            "file://{}",
            CursorFile::img_path(&self.recording_file, hash)
        );
        let img_asset =
            ges::UriClipAsset::request_sync(img_uri.as_ref()).expect("cant find cursor image file");
        let img_info = img_asset.info().video_streams()[0].clone();

        let img_clip = match layer.add_asset(
            &img_asset,
            gst::ClockTime::ZERO,
            gst::ClockTime::ZERO,
            self.video_duration,
            ges::TrackType::VIDEO,
        ) {
            Ok(clip) => clip,
            Err(err) => {
                tracing::error!("failed to add cursor {}: {}", hash, err.message);
                return Ok(());
            }
        };

        let cursor_width = img_info.width() as f64 * 3.0;
        let cursor_height = img_info.height() as f64 * 3.0;
        img_clip.set_child_property("fwidth", cursor_width)?;
        img_clip.set_child_property("fheight", cursor_height)?;

        let track_element = img_clip
            .children(true)
            .into_iter()
            .find(|child| child.is::<ges::VideoUriSource>())
            .expect("failed to find video in image clip");
        let cursor_cs = CursorControlSources {
            posx: gst_controller::InterpolationControlSource::new(),
            posy: gst_controller::InterpolationControlSource::new(),
            alpha: gst_controller::InterpolationControlSource::new(),
        };
        cursor_cs
            .posx
            .set_property("mode", gst_controller::InterpolationMode::CubicMonotonic);
        cursor_cs
            .posy
            .set_property("mode", gst_controller::InterpolationMode::CubicMonotonic);

        if let Ok(track_element) = track_element.clone().dynamic_cast::<ges::TrackElement>() {
            track_element.set_control_source(&cursor_cs.posx, "posx", "direct-absolute");
            track_element.set_control_source(&cursor_cs.posy, "posy", "direct-absolute");
            track_element.set_control_source(&cursor_cs.alpha, "alpha", "direct-absolute");
        }

        self.cursor_cs.insert(hash, cursor_cs);
        self.cursor_layers.insert(hash, layer);

        Ok(())
    }

    // FIX: no cursor at beginning
    // PERF: freeze
    fn update_cursor_types(&self) {
        for (_, cs) in self.cursor_cs.iter() {
            cs.alpha.set(gst::ClockTime::ZERO, 0.0);
        }
        let mut prev_hash: Option<u64> = None;
        for cursor in self.cursor_entries.iter() {
            let Some(hash) = cursor.cursor_type_hash else {
                continue;
            };

            let clocktime = gst::ClockTime::from_nseconds(cursor.pts as u64);

            if let Some(prev_hash) = prev_hash
                && let Some(prev) = self.cursor_cs.get(&prev_hash)
            {
                if prev_hash == hash {
                    continue;
                }
                prev.alpha.set(clocktime, 0.0);
            }

            if let Some(cur) = self.cursor_cs.get(&hash) {
                cur.alpha.set(clocktime, 1.0);
                prev_hash = Some(hash);
            }
        }
    }

    fn foreach_frame<F>(&self, run: F)
    where
        F: Fn(gst::ClockTime, u64),
    {
        let Some(video_duration) = self.video_duration else {
            return;
        };

        let fps = self.video_framerate.unwrap().numer() as f32
            / self.video_framerate.unwrap().denom() as f32;
        let duration_ns = video_duration.nseconds();
        let frame_duration_ns = (1_000_000_000.0 / fps) as u64;
        let frame_duration_clocktime = gst::ClockTime::from_nseconds(frame_duration_ns);
        let frames = duration_ns / frame_duration_ns;
        let mut current_clocktime = gst::ClockTime::ZERO;
        let mut current_pts = 0;
        for _ in 0..frames {
            run(current_clocktime, current_pts);
            current_clocktime.add_assign(frame_duration_clocktime);
            current_pts += frame_duration_ns;
        }
    }

    pub fn redraw_cursor(&mut self) {
        for (_, cs) in self.cursor_cs.iter() {
            self.redraw_cursor_type(cs);
        }
    }

    fn redraw_cursor_type(&self, cursor_cs: &CursorControlSources) {
        if !self.cursor_enabled {
            return;
        }

        self.foreach_frame(|current_clocktime, current_pts| {
            let (curr_x, curr_y) = rdp::get_position_at_time(
                &self.cursor_entries,
                &self.cursor_used_entries,
                current_pts as f64,
            );

            let top = self.zoom_value_at("top", current_clocktime).unwrap_or(0.0);
            let bottom = self
                .zoom_value_at("bottom", current_clocktime)
                .unwrap_or(0.0);
            let left = self.zoom_value_at("left", current_clocktime).unwrap_or(0.0);
            let right = self
                .zoom_value_at("right", current_clocktime)
                .unwrap_or(0.0);

            let visible_width = self.video_width as f64 - left - right;
            let visible_height = self.video_height as f64 - top - bottom;
            let scale_x = self.video_width as f64 / visible_width;
            let scale_y = self.video_height as f64 / visible_height;

            // TODO: add offset
            let final_x = (curr_x - left) * scale_x;
            let final_y = (curr_y - top) * scale_y;
            cursor_cs.posx.set(current_clocktime, final_x);
            cursor_cs.posy.set(current_clocktime, final_y);
        });
    }

    fn update_cursor_smoothing(&mut self) {
        self.cursor_used_entries = rdp::ramer_douglas_peucker(
            &self.cursor_entries,
            0,
            self.cursor_entries.len() - 1,
            self.cursor_smoothing,
        );
    }

    fn setup_zoom(
        videobox_effect: &ges::Effect,
        video_duration: Option<gst::ClockTime>,
    ) -> Result<ZoomControlSourcesArray> {
        let mut props: ZoomControlSourcesArray = [
            ("top", None),
            ("bottom", None),
            ("left", None),
            ("right", None),
        ];

        let duration = video_duration
            .or_else(|| {
                tracing::warn!("undefined video duration");
                Some(gst::ClockTime::MAX)
            })
            .unwrap();

        let track_element: ges::TrackElement = videobox_effect.clone().upcast();
        track_element
            .nleobject()
            .set_control_rate(gst::ClockTime::from_mseconds(33));

        for (prop_name, control_source) in props.iter_mut() {
            let cs = gst_controller::InterpolationControlSource::new();
            cs.set_property("mode", gst_controller::InterpolationMode::Linear);

            cs.set(gst::ClockTime::ZERO, 0.0);
            cs.set(duration, 0.0);

            track_element.set_control_source(&cs, prop_name, "direct-absolute");

            *control_source = Some(cs);
        }

        Ok(props)
    }

    pub fn add_zoom(&mut self, zoom: ZoomEffect) -> Result<()> {
        if let Err(e) = self.show_zoom(&zoom) {
            bail!(e);
        }
        self.zoom_effects.push(zoom);
        self.redraw_cursor();
        Ok(())
    }

    fn show_zoom(&self, zoom: &ZoomEffect) -> Result<()> {
        let crop_total_x = self.video_width as f64 * (1.0 - 1.0 / zoom.factor);
        let crop_total_y = self.video_height as f64 * (1.0 - 1.0 / zoom.factor);

        let left = crop_total_x * (1.0 - zoom.pos_x);
        let right = crop_total_x * zoom.pos_x;
        let top = crop_total_y * (1.0 - zoom.pos_y);
        let bottom = crop_total_y * zoom.pos_y;

        let values = [
            ("top", top),
            ("bottom", bottom),
            ("left", left),
            ("right", right),
        ];

        let (start_anim_ns, start_ns, end_ns, end_anim_ns) = zoom.clocktimes(self.duration_nsec());

        for (prop_name, cs) in self.zoom_cs.iter() {
            let Some(cs) = cs else {
                tracing::warn!("undefined control source for zoom property {}", prop_name);
                continue;
            };

            let final_val = *values
                .iter()
                .find_map(|(prop, cs)| (*prop == *prop_name).then_some(cs))
                .unwrap();

            let Some(start_val) = gst::prelude::ControlSourceExt::value(cs, gst::ClockTime::ZERO)
            else {
                tracing::warn!("undefined default value for zoom property {}", prop_name);
                continue;
            };

            if !start_anim_ns.is_zero() {
                cs.set(start_anim_ns, start_val);
            }
            cs.set(start_ns, final_val);
            cs.set(end_ns, final_val);
            cs.set(end_anim_ns, start_val);
        }

        Ok(())
    }

    pub fn update_zoom_geometry(
        &mut self,
        zoom_id: usize,
        factor: f64,
        pos_x: f64,
        pos_y: f64,
    ) -> Result<()> {
        {
            let Some(zoom) = self.zoom_effect_mut(zoom_id) else {
                bail!("cant find zoom with id {}", zoom_id)
            };
            zoom.factor = factor;
            zoom.pos_x = pos_x;
            zoom.pos_y = pos_y;
        }
        if let Some(zoom) = self.zoom_effect(zoom_id) {
            self.show_zoom(zoom)?;
            self.redraw_cursor();
        };
        Ok(())
    }

    pub fn update_zoom_range(
        &mut self,
        zoom_id: usize,
        previous_start_nsec: u64,
        previous_end_nsec: u64,
    ) -> Result<()> {
        {
            let Some(zoom) = self.zoom_effect_mut(zoom_id) else {
                bail!("cant find zoom with id {}", zoom_id)
            };
            if zoom.start_nsec == previous_start_nsec && zoom.end_nsec == previous_end_nsec {
                return Ok(());
            }

            // recreate the previous zoom to unshow it
            let mut zoom = zoom.clone();
            zoom.start_nsec = previous_start_nsec;
            zoom.end_nsec = previous_end_nsec;
            self.unshow_zoom(&zoom)?;
        }

        if let Some(zoom) = self.zoom_effect(zoom_id) {
            self.show_zoom(zoom)?;
            self.redraw_cursor();
        }

        Ok(())
    }

    pub fn update_cursor_size(&mut self, size: u32) -> Result<()> {
        for (_, layer) in self.cursor_layers.iter() {
            let clips = layer.clips();
            let Some(clip) = clips.iter().next() else {
                continue;
            };
            let Some(asset) = clip.asset() else {
                continue;
            };
            let uriclip_asset = asset
                .downcast_ref::<ges::UriClipAsset>()
                .ok_or_else(|| anyhow::anyhow!("not a UriClipAsset"))?;
            let video_info = uriclip_asset.info().video_streams()[0].clone();
            clip.set_child_property("fwidth", video_info.width() * size)?;
            clip.set_child_property("fheight", video_info.width() * size)?;
        }
        self.timeline().commit();
        Ok(())
    }

    pub fn remove_zoom(&mut self, zoom_id: usize) -> Result<()> {
        let Some(zoom) = self.zoom_effect(zoom_id) else {
            bail!("cant find zoom with id {}", zoom_id);
        };

        self.unshow_zoom(zoom)?;

        self.zoom_effects.remove(zoom_id);

        self.redraw_cursor();

        Ok(())
    }

    fn unshow_zoom(&self, zoom: &ZoomEffect) -> Result<()> {
        let (start_anim_ns, start_ns, end_ns, end_anim_ns) = zoom.clocktimes(self.duration_nsec());

        for (prop_name, cs) in self.zoom_cs.iter() {
            let Some(cs) = cs else {
                tracing::warn!("undefined control source for zoom property {}", prop_name);
                continue;
            };

            if !start_anim_ns.is_zero() {
                cs.unset(start_anim_ns);
            }
            cs.unset(start_ns);
            cs.unset(end_ns);
            cs.unset(end_anim_ns);
        }
        Ok(())
    }

    pub fn zoom_effects(&self) -> Vec<ZoomEffect> {
        self.zoom_effects.clone()
    }

    pub fn zoom_effect_mut(&mut self, effect_index: usize) -> Option<&mut ZoomEffect> {
        self.zoom_effects.get_mut(effect_index)
    }

    pub fn zoom_effect(&self, effect_index: usize) -> Option<&ZoomEffect> {
        self.zoom_effects.get(effect_index)
    }

    pub fn apply_zoom_effects(&mut self, effects: &Vec<ZoomEffect>) -> Result<()> {
        for effect in effects {
            self.show_zoom(effect)?;
        }
        Ok(())
    }
}

fn read_cursor_entries(path: &str) -> anyhow::Result<(Vec<CursorEntry>, Vec<usize>)> {
    let mut file = File::open(path)?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)?;

    let mut hashes: HashSet<u64> = HashSet::new();
    let mut types: Vec<usize> = Vec::new();
    let mut i = 0;
    let entries: Vec<CursorEntry> = data
        .chunks_exact(24)
        .map(|chunk| {
            let raw_hash = u64::from_le_bytes(chunk[16..24].try_into().unwrap());
            let cursor_type_hash = if raw_hash == 0 { None } else { Some(raw_hash) };

            if let Some(hash) = cursor_type_hash
                && hashes.insert(hash)
            {
                types.push(i);
            }

            i += 1;

            CursorEntry::new(
                u64::from_le_bytes(chunk[0..8].try_into().unwrap()),
                i32::from_le_bytes(chunk[8..12].try_into().unwrap()),
                i32::from_le_bytes(chunk[12..16].try_into().unwrap()),
                cursor_type_hash,
            )
        })
        .collect();

    Ok((entries, types))
}

fn video_track_element(video_uriclip: &ges::UriClip) -> ges::TrackElement {
    video_uriclip
        .children(true)
        .into_iter()
        .filter_map(|child| child.downcast::<ges::TrackElement>().ok())
        .find(|element| element.track_type() == ges::TrackType::VIDEO)
        .expect("failed to find track element in video clip")
}
