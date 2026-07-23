use glib::Object;
use glib::subclass::types::ObjectSubclassIsExt;
use gtk::Application;
use gtk::gio;
use gtk::glib;
use std::path::PathBuf;

mod imp {
    use crate::config::APP_ID;
    use crate::config::PROFILE;
    use crate::core::render;
    use crate::core::utils;
    use crate::core::video::Video;
    use crate::core::video::ZoomEffect;
    use crate::widgets::timecode;
    use crate::widgets::timeline;
    use crate::windows;
    use adw::prelude::*;
    use adw::subclass::prelude::*;
    use anyhow::Result;
    use ges::prelude::TimelineExt;
    use ges::prelude::*;
    use gettextrs::gettext;
    use gio::glib::property::PropertySet;
    use glib::subclass::InitializingObject;
    use glib::subclass::types::ObjectSubclassIsExt;
    use gst::prelude::ElementExt;
    use gtk::CompositeTemplate;
    use gtk::gio;
    use gtk::glib;
    use std::cell::OnceCell;
    use std::cell::Ref;
    use std::cell::RefCell;
    use std::cell::RefMut;
    use std::path::PathBuf;
    use std::rc::Rc;

    #[derive(CompositeTemplate, Debug, Default)]
    #[template(resource = "/fr/alexpiquard/ferricast/ui/editor.ui")]
    pub struct EditorWindow {
        #[template_child]
        pub video_area: TemplateChild<gtk::Picture>,
        #[template_child]
        pub timeline_widget: TemplateChild<timeline::Timeline>,
        #[template_child]
        pub cursor_page_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub cursor_smoothing_scale: TemplateChild<gtk::Scale>,
        #[template_child]
        pub cursor_show: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub cursor_size_spin: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub render_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub pause_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub timecode_widget: TemplateChild<timecode::Timecode>,
        pub video: OnceCell<Rc<RefCell<Video>>>,
        render_settings: RefCell<render::RenderSettings>,
        settings: OnceCell<gio::Settings>,
        // TODO: choose cursor file and adjust pointer (0 -> 1), or predefined cursors
        // -> one cursor per state : input, drag, normal
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EditorWindow {
        const NAME: &'static str = "EditorWindow";
        type Type = super::EditorWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for EditorWindow {
        fn constructed(&self) {
            self.parent_constructed();

            if *PROFILE == "Devel" {
                self.obj().add_css_class("devel");
            }

            self.settings
                .set(gio::Settings::new(*APP_ID))
                .expect("failed to set settings");
            self.load_window_size();
        }
    }

    impl WindowImpl for EditorWindow {
        fn close_request(&self) -> glib::Propagation {
            if let Err(err) = self.save_window_size() {
                tracing::warn!("Failed to save window state, {}", &err);
            }

            self.parent_close_request()
        }
    }

    impl WidgetImpl for EditorWindow {}

    impl ApplicationWindowImpl for EditorWindow {}

    impl AdwApplicationWindowImpl for EditorWindow {}

    #[gtk::template_callbacks]
    impl EditorWindow {
        #[template_callback]
        fn handle_pause_clicked(&self, button: &gtk::Button) {
            let pipeline = self.video().pipeline();
            let (_, state, _) = pipeline.state(gst::ClockTime::ZERO);
            let (state, icon) = if state == gst::State::Paused {
                (gst::State::Playing, "media-playback-pause-symbolic")
            } else {
                (gst::State::Paused, "media-playback-start-symbolic")
            };
            if let Err(e) = pipeline.set_state(state) {
                tracing::error!("failed to change pipeline state to {:?}: {:#?}", state, e);
            } else {
                button.set_icon_name(icon);
            }
        }

        #[template_callback]
        fn handle_render_clicked(&self) {
            let window = windows::RenderWindow::new(&self.obj(), &self.render_settings.borrow());
            window.set_transient_for(Some(&*self.obj()));
            window.present();
        }

        #[template_callback]
        fn handle_zoom_clicked(&self) {
            let Some(current_pos) = self.video().current_position_nsec() else {
                tracing::error!("failed to get current timeline position");
                return;
            };

            if let Err(err) = self.video_mut().add_zoom(ZoomEffect {
                start_nsec: current_pos,
                end_nsec: current_pos + 2_000_000_000,
                ..ZoomEffect::default()
            }) {
                tracing::error!("failed to add zoom: {:#?}", err);
                return;
            }

            self.video().timeline().commit();
        }

        #[template_callback]
        fn handle_cursor_smoothing_changed(&self, scale: &gtk::Scale) {
            if !self.is_setup() {
                return;
            }
            self.video_mut().set_cursor_smoothing(scale.value());
        }

        #[template_callback]
        fn handle_cursor_switch(&self, _pspec: glib::ParamSpec, switch: &adw::SwitchRow) {
            if !self.is_setup() {
                return;
            }
            if let Err(e) = self.video_mut().set_cursor_show(switch.is_active()) {
                tracing::error!("failed to hide/show cursor: {:?}", e);
            }
        }

        #[template_callback]
        fn handle_cursor_size_changed(&self, spin: &adw::SpinRow) {
            if let Err(e) = self.video_mut().update_cursor_size(spin.value() as u32) {
                tracing::error!("failed to change cursor size: {:?}", e);
            }
        }
    }

    impl EditorWindow {
        pub fn setup(&self, recording_file: PathBuf) {
            if let Err(e) = self.setup_video(recording_file) {
                tracing::error!("failed to setup player pipeline: {:?}", e);
                utils::show_error_dialog_and_close(
                    &*self.obj(),
                    &gettext("Failed to open recording"),
                    &format!("{}", e),
                );
                return;
            };

            self.idle_setup();
        }

        fn idle_setup(&self) {
            let this = self.obj().downgrade();
            let video_rc = self.video_rc();
            glib::idle_add_local(move || {
                let Some(this) = this.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                if let Err(e) = video_rc.borrow_mut().setup_cursor() {
                    tracing::warn!("failed to setup video cursor: {:?}", e);
                }

                this.imp().timeline_widget.imp().setup(video_rc.clone());
                this.imp().timecode_widget.imp().setup(video_rc.clone());
                this.imp().set_initial_values();
                video_rc.borrow().start();
                this.imp().after_setup();
                glib::ControlFlow::Break
            });
        }

        fn is_setup(&self) -> bool {
            self.render_button.is_sensitive()
        }

        fn after_setup(&self) {
            self.render_button.set_sensitive(true);
            self.pause_button.set_sensitive(true);
        }

        pub fn video_rc(&self) -> Rc<RefCell<Video>> {
            self.video.get().cloned().expect("undefined video")
        }

        pub fn video(&self) -> Ref<'_, Video> {
            self.video.get().expect("undefined video").borrow()
        }

        pub fn video_mut(&self) -> RefMut<'_, Video> {
            self.video.get().expect("undefined video").borrow_mut()
        }

        fn setup_video(&self, recording_file: PathBuf) -> Result<()> {
            let sink = gst::ElementFactory::make("gtk4paintablesink")
                .build()
                .unwrap();
            let queue = gst::ElementFactory::make("queue").build().unwrap();
            let convert = gst::ElementFactory::make("videoconvertscale")
                .build()
                .unwrap();

            queue.set_property("max-size-buffers", 2u32);

            let bin = gst::Bin::new();
            bin.add_many([&queue, &convert, &sink]).unwrap();

            gst::Element::link_many([&queue, &convert, &sink]).unwrap();

            let pad = queue.static_pad("sink").unwrap();
            let ghost_pad = gst::GhostPad::with_target(&pad).unwrap();
            bin.add_pad(&ghost_pad).unwrap();

            let paintable = sink.property::<gtk::gdk::Paintable>("paintable");

            let this = self.obj().clone();
            let video = Video::try_new(
                recording_file,
                Some(move |enabled| {
                    this.imp().cursor_show.set_sensitive(enabled);
                    this.imp().cursor_smoothing_scale.set_sensitive(enabled);
                    this.imp().cursor_size_spin.set_sensitive(enabled);

                    if let Some(row) = this
                        .imp()
                        .cursor_smoothing_scale
                        .ancestor(adw::ActionRow::static_type())
                        .and_downcast::<adw::ActionRow>()
                    {
                        row.set_sensitive(enabled);
                    }

                    this.imp().cursor_page_banner.set_revealed(!enabled);
                    if !enabled {
                        this.imp()
                            .cursor_page_banner
                            .set_title("Cursor file not found, related features are disabled");
                    }
                }),
            )?;
            video.set_video_sink(&bin);
            self.video
                .set(Rc::new(RefCell::new(video)))
                .expect("failed to set video");
            self.video_area.set_paintable(Some(&paintable));

            Ok(())
        }

        fn set_initial_values(&self) {
            let video = self.video();
            self.cursor_smoothing_scale
                .set_value(video.cursor_smoothing());
            self.cursor_show.set_active(video.cursor_show());
        }

        pub fn save_render_settings(&self, settings: render::RenderSettings) {
            self.render_settings.set(settings);
        }

        fn settings(&self) -> &gio::Settings {
            self.settings.get().expect("undefined settings")
        }

        fn save_window_size(&self) -> Result<(), glib::BoolError> {
            let (width, height) = self.obj().default_size();

            self.settings().set_int("window-width", width)?;
            self.settings().set_int("window-height", height)?;

            self.settings()
                .set_boolean("is-maximized", self.obj().is_maximized())?;

            Ok(())
        }

        fn load_window_size(&self) {
            let width = self.settings().int("window-width");
            let height = self.settings().int("window-height");
            let is_maximized = self.settings().boolean("is-maximized");

            self.obj().set_default_size(width, height);

            if is_maximized {
                self.obj().maximize();
            }
        }
    }
}

glib::wrapper! {
    pub struct EditorWindow(ObjectSubclass<imp::EditorWindow>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, adw::Window, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl EditorWindow {
    pub fn new(app: &Application, recording_file: PathBuf) -> Self {
        let window: Self = Object::builder().property("application", app).build();
        window.imp().setup(recording_file);
        window
    }
}
