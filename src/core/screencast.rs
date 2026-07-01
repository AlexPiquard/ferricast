use std::{
    cell::RefCell,
    os::fd::OwnedFd,
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc,
    time::{SystemTime, UNIX_EPOCH},
};

use gst::{bus::BusWatchGuard, prelude::*};

use ashpd::desktop::{
    PersistMode,
    screencast::{CursorMode, Screencast, SourceType},
};
use pipewire::{self as pw, spa::utils::Rectangle, stream::StreamState};
use pw::{properties::properties, spa};

use crate::core::{cursor::CursorFile, utils};

struct UserData {
    format: spa::param::video::VideoInfoRaw,
    video_appsrc: Option<gst_app::AppSrc>,
    bus_guard: Option<BusWatchGuard>,
    temp_filename: Rc<PathBuf>,
    cursor_file: Option<RefCell<CursorFile>>,
    cursor_hashes: Vec<u64>,
    pipeline: Option<gst::Pipeline>,
    first_buffer_pts: u64,
    first_cursor_pts: u64,
    last_cursor_pts: u64,
    last_buffer_pts: u64,
    last_frame_data: Option<Vec<u8>>,
    eos_sender: mpsc::Sender<()>,
}

impl UserData {
    fn error(&self, msg: &str) {
        tracing::error!("{}", msg);
        let _ = self.eos_sender.send(());
    }
}

pub async fn prepare_screencast() -> ashpd::Result<(u32, OwnedFd, PathBuf)> {
    let screencast_proxy = Screencast::new().await?;
    let session = screencast_proxy.create_session().await?;
    screencast_proxy
        .select_sources(
            &session,
            CursorMode::Metadata,
            SourceType::Monitor | SourceType::Window,
            true,
            None,
            PersistMode::DoNot,
        )
        .await?;

    let response = screencast_proxy.start(&session, None).await?.response()?;
    let streams = response.streams();
    let stream_info = streams
        .first()
        .ok_or_else(|| ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled))?;

    let fd = screencast_proxy.open_pipe_wire_remote(&session).await?;

    Ok((
        stream_info.pipe_wire_node_id(),
        fd,
        generate_temp_filename(),
    ))
}

/// Generates a unique temporary filename for recording
fn generate_temp_filename() -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    utils::tmp_dir(None).join(format!("recording_{}.mkv", timestamp))
}

fn create_pipeline(
    size: Rectangle,
    temp_filename: &Path,
) -> anyhow::Result<(gst::Pipeline, gst_app::AppSrc)> {
    let pipeline = gst::Pipeline::default();

    let video_info =
        gst_video::VideoInfo::builder(gst_video::VideoFormat::Bgrx, size.width, size.height)
            .fps(gst::Fraction::new(30, 1))
            .build()
            .expect("failed to create video info");

    let video_appsrc = gst_app::AppSrc::builder()
        .caps(&video_info.to_caps().unwrap())
        .format(gst::Format::Time)
        .is_live(true)
        .do_timestamp(true)
        .build();

    let videoconvert = gst::ElementFactory::make("videoconvert").build()?;
    let x264enc = gst::ElementFactory::make("x264enc").build()?;

    let h264parse = gst::ElementFactory::make("h264parse").build()?;
    let muxer = gst::ElementFactory::make("matroskamux").build()?;
    let sink = gst::ElementFactory::make("filesink")
        .property("location", temp_filename.to_str().unwrap())
        .property("sync", false)
        .build()?;

    pipeline.add_many([
        video_appsrc.upcast_ref(),
        &videoconvert,
        &x264enc,
        &h264parse,
        &muxer,
        &sink,
    ])?;
    gst::Element::link_many([
        video_appsrc.upcast_ref(),
        &videoconvert,
        &x264enc,
        &h264parse,
        &muxer,
        &sink,
    ])?;

    anyhow::Ok((pipeline, video_appsrc))
}

fn start_pipeline(
    pipeline: &gst::Pipeline,
    sender: mpsc::Sender<()>,
) -> anyhow::Result<BusWatchGuard> {
    pipeline
        .set_state(gst::State::Playing)
        .expect("failed to set playing state");

    tracing::info!("Started pipeline bus");

    let bus = pipeline.bus().expect("failed to get pipeline bus");

    let guard = bus.add_watch(glib::clone!(
        #[strong]
        sender,
        #[strong]
        pipeline,
        move |_bus, msg| {
            use gst::MessageView;

            match msg.view() {
                MessageView::Error(e) => {
                    let _ = sender.send(());
                    pipeline
                        .set_state(gst::State::Null)
                        .expect("failed to set null state");
                    tracing::error!("GStreamer error: {:?}", e.debug());
                    return glib::ControlFlow::Break;
                }
                MessageView::Eos(_) => {
                    let _ = sender.send(());
                    pipeline
                        .set_state(gst::State::Null)
                        .expect("failed to set null state");
                    return glib::ControlFlow::Break;
                }
                _ => (),
            }

            glib::ControlFlow::Continue
        }
    ))?;

    anyhow::Ok(guard)
}

pub fn start_screencast(node_id: u32, fd: OwnedFd, temp_filename: PathBuf) -> anyhow::Result<()> {
    pw::init();
    tracing::info!("node_id={}", node_id);

    let mainloop = Rc::new(pw::main_loop::MainLoopBox::new(None)?);
    let context = pw::context::ContextBox::new(mainloop.loop_(), None)?;
    let core = context.connect_fd(fd, None)?;

    // create channel for communication between pipeline thread and main thread
    let (sender, receiver) = mpsc::channel::<()>();

    let temp_filename = Rc::new(temp_filename);
    let data = UserData {
        format: Default::default(),
        video_appsrc: None,
        bus_guard: None,
        temp_filename,
        cursor_file: None,
        cursor_hashes: Vec::new(),
        pipeline: None,
        first_cursor_pts: 0,
        first_buffer_pts: 0,
        last_cursor_pts: 0,
        last_buffer_pts: 0,
        last_frame_data: None,
        eos_sender: sender,
    };

    let stream = pw::stream::StreamBox::new(
        &core,
        "streambox",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::NODE_NAME => "MonCapteurEcran",
            *pw::keys::MEDIA_ROLE => "ScreenCapture",
        },
    )?;

    let _listener = stream
        .add_local_listener_with_user_data(data)
        .state_changed(move |_, user_data, old, new| {
            if old != StreamState::Streaming || new != StreamState::Paused {
                return;
            }

            let expected_duration_ns = user_data.last_cursor_pts;
            let expected_pts = gst::ClockTime::from_nseconds(expected_duration_ns);

            // push a final frame to establish duration
            if let Some(frame_data) = user_data.last_frame_data.as_ref() {
                let mut gst_buffer = gst::Buffer::from_slice(frame_data.clone());
                if let Some(buf) = gst_buffer.get_mut() {
                    buf.set_pts(expected_pts);
                }

                if let Err(e) = user_data
                    .video_appsrc
                    .as_ref()
                    .unwrap()
                    .push_buffer(gst_buffer)
                {
                    tracing::warn!("GStreamer error when pushing last video buffer: {:?}", e);
                } else {
                    tracing::debug!("pushed last buffer (pts={:?})", expected_pts);
                }
            }

            if let Err(e) = user_data.video_appsrc.as_ref().unwrap().end_of_stream() {
                tracing::warn!("failed to send end of stream event: {:?}", e);
            }
        })
        .param_changed(move |_, user_data, id, param| {
            let Some(param) = param else {
                return;
            };
            if id != pw::spa::param::ParamType::Format.as_raw() {
                return;
            }

            let (media_type, media_subtype) =
                match pw::spa::param::format_utils::parse_format(param) {
                    Ok(v) => v,
                    Err(_) => return,
                };

            if media_type != pw::spa::param::format::MediaType::Video
                || media_subtype != pw::spa::param::format::MediaSubtype::Raw
            {
                return;
            }

            if let Err(e) = user_data.format.parse(param) {
                user_data.error(&format!(
                    "failed to parse param changed to VideoInfoRaw: {}",
                    e
                ));
                return;
            }

            tracing::debug!("pipewire format : {:?}", user_data.format);

            let size = user_data.format.size();

            let Ok((pipeline, video_appsrc)) = create_pipeline(size, &user_data.temp_filename)
            else {
                user_data.error("failed to create pipeline");
                return;
            };

            user_data.video_appsrc = Some(video_appsrc);
            user_data.pipeline = Some(pipeline.clone());
            user_data.cursor_file = Some(RefCell::new(CursorFile::new(&user_data.temp_filename)));

            match start_pipeline(
                user_data.pipeline.as_ref().unwrap(),
                user_data.eos_sender.clone(),
            ) {
                Ok(bus_guard) => user_data.bus_guard = Some(bus_guard),
                Err(e) => {
                    user_data.error(&format!("failed to start pipeline: {}", e));
                }
            }
        })
        .process(|stream, user_data| {
            let raw_buf;

            unsafe {
                raw_buf = stream.dequeue_raw_buffer();
            }

            if raw_buf.is_null() {
                return;
            }

            // TODO: try to remove unsafe
            unsafe {
                let spa_buf = (*raw_buf).buffer;

                let data_info = (*spa_buf).datas.offset(0);
                let chunk = *(*data_info).chunk;
                let size = chunk.size as usize;

                let metas_ptr = (*spa_buf).metas;
                let mut pts: u64 = 0;
                let mut cursor: Option<*const spa::sys::spa_meta_cursor> = None;
                let mut cursor_hash: Option<u64> = None;
                for i in 0..(*spa_buf).n_metas {
                    let meta = metas_ptr.add(i as usize);

                    let m_type = (*meta).type_;
                    let m_data = (*meta).data;

                    if m_type == spa::sys::SPA_META_Header {
                        let header = m_data as *const spa::sys::spa_meta_header;
                        pts = (*header).pts as u64;
                    }
                    if m_type == spa::sys::SPA_META_Cursor {
                        cursor = Some(m_data as *const spa::sys::spa_meta_cursor);

                        if let Some(c) = cursor
                            && (*c).bitmap_offset != 0
                        {
                            let bitmap_ptr = (c as *const u8).add((*c).bitmap_offset as usize)
                                as *const spa::sys::spa_meta_bitmap;
                            let bitmap = *bitmap_ptr;

                            let width = bitmap.size.width;
                            let height = bitmap.size.height;
                            let stride = bitmap.stride;

                            if width > 0 && height > 0 && bitmap.offset != 0 {
                                let pixels_ptr =
                                    (bitmap_ptr as *const u8).add(bitmap.offset as usize);

                                let total_bytes = (height * stride as u32) as usize;
                                let pixels_slice =
                                    std::slice::from_raw_parts(pixels_ptr, total_bytes);

                                cursor_hash = Some(hash_pixels(pixels_slice));

                                if !user_data.cursor_hashes.contains(&cursor_hash.unwrap()) {
                                    let pixels_data = pixels_slice.to_vec();

                                    // TODO : bitmap position
                                    if let Some(image_buffer) = image::ImageBuffer::<
                                        image::Rgba<u8>,
                                        _,
                                    >::from_raw(
                                        width, height, pixels_data
                                    ) {
                                        match image_buffer.save(CursorFile::img_path(
                                            &user_data.temp_filename,
                                            cursor_hash.unwrap(),
                                        )) {
                                            Ok(_) => {
                                                user_data.cursor_hashes.push(cursor_hash.unwrap());
                                            }
                                            Err(e) => tracing::error!(
                                                "failed to save cursor image : {:?}",
                                                e
                                            ),
                                        }
                                    } else {
                                        tracing::error!("failed to crate image buffer for cursor");
                                    }
                                }
                            }
                        }
                    }
                }

                if pts == 0 {
                    user_data.error("missing pts header");
                    return;
                }

                let mut relative_pts = 0;
                let mut gst_pts: gst::ClockTime = gst::ClockTime::default();
                if user_data.first_cursor_pts > 0 {
                    relative_pts = pts.saturating_sub(user_data.first_cursor_pts);
                    gst_pts = gst::ClockTime::from_nseconds(relative_pts);
                }
                if size > 0 && !(*data_info).data.is_null() {
                    let offset = chunk.offset as usize;
                    let ptr = ((*data_info).data as *const u8).add(offset);

                    let frame_data = std::slice::from_raw_parts(ptr, size).to_vec();
                    user_data.last_frame_data = Some(frame_data.clone());

                    if user_data.first_buffer_pts == 0 {
                        user_data.first_buffer_pts = pts;
                        user_data.first_cursor_pts = pts;
                    }
                    user_data.last_buffer_pts = relative_pts;

                    let Ok(mut gst_buffer) = gst::Buffer::with_size(size) else {
                        tracing::warn!("failed to allocate gst buffer");
                        return;
                    };
                    if let Some(gst_buffer_mut) = gst_buffer.get_mut() {
                        gst_buffer_mut.set_pts(gst_pts);
                        let mut map = gst_buffer_mut.map_writable().unwrap();
                        map.as_mut_slice().copy_from_slice(&frame_data);
                    }

                    if let Err(e) = user_data
                        .video_appsrc
                        .as_ref()
                        .unwrap()
                        .push_buffer(gst_buffer)
                    {
                        tracing::warn!("GStreamer error when pushing video buffer: {:?}", e);
                    } else {
                        tracing::debug!("pushed buffer (pts={:?})", gst_pts);
                    }
                }

                if let Some(cursor) = cursor
                    && let Some(cursor_file) = user_data.cursor_file.as_ref()
                    && user_data.first_buffer_pts > 0
                {
                    user_data.last_cursor_pts = relative_pts;

                    tracing::debug!(
                        "cursor x={} y={} (pts={:?}) {:?}",
                        (*cursor).position.x,
                        (*cursor).position.y,
                        gst::ClockTime::from_nseconds(relative_pts),
                        *cursor
                    );
                    let mut file = cursor_file.borrow_mut();
                    // TODO: calculate hash of bitmap to identify it, create corresponding png's
                    // FIX: incorrect position in window if really out of window
                    file.write(
                        relative_pts,
                        (*cursor).position.x,
                        (*cursor).position.y,
                        cursor_hash,
                    );
                }
            }

            unsafe {
                stream.queue_raw_buffer(raw_buf);
            }
        })
        .register()?;

    tracing::info!("Created stream");

    let obj = pw::spa::pod::object!(
        pw::spa::utils::SpaTypes::ObjectParamFormat,
        pw::spa::param::ParamType::EnumFormat,
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaType,
            Id,
            pw::spa::param::format::MediaType::Video
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaSubtype,
            Id,
            pw::spa::param::format::MediaSubtype::Raw
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFormat,
            Id,
            pw::spa::param::video::VideoFormat::BGRx
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            pw::spa::utils::Fraction { num: 25, denom: 1 },
            pw::spa::utils::Fraction { num: 0, denom: 1 },
            pw::spa::utils::Fraction {
                num: 1000,
                denom: 1
            }
        )
    );

    let mut params = [
        pod_from_object(obj),
        create_meta_pod(spa::sys::SPA_META_Cursor),
        create_meta_pod(spa::sys::SPA_META_Header),
    ];

    stream.connect(
        spa::utils::Direction::Input,
        Some(node_id),
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;

    tracing::info!("Connected stream");

    // listen for messages from gst pipeline
    loop {
        match receiver.try_recv() {
            Ok(()) => {
                mainloop.quit();
                break;
            }
            Err(mpsc::TryRecvError::Empty) => (),
            Err(mpsc::TryRecvError::Disconnected) => {
                tracing::warn!("Pipeline thread disconnected, quitting");
                mainloop.quit();
                break;
            }
        }

        // check if mainloop should still run
        if mainloop
            .loop_()
            .iterate(std::time::Duration::from_millis(10))
            < 0
        {
            break;
        }
    }

    Ok(())
}

fn pod_from_object(object: spa::pod::Object) -> &'static spa::pod::Pod {
    let raw_pod: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(object),
    )
    .unwrap()
    .0
    .into_inner();

    let leaked_bytes: &'static [u8] = Box::leak(raw_pod.into_boxed_slice());
    spa::pod::Pod::from_bytes(leaked_bytes).unwrap()
}

fn create_meta_pod(value_id: u32) -> &'static spa::pod::Pod {
    let obj = spa::pod::Object {
        type_: spa::sys::SPA_TYPE_OBJECT_ParamMeta,
        id: spa::sys::SPA_PARAM_Meta,
        properties: vec![spa::pod::Property {
            key: spa::sys::SPA_PARAM_META_type,
            flags: spa::pod::PropertyFlags::empty(),
            value: spa::pod::Value::Id(spa::utils::Id(value_id)),
        }],
    };
    pod_from_object(obj)
}

fn hash_pixels(pixels: &[u8]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    pixels.hash(&mut hasher);
    hasher.finish()
}
