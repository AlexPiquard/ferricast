use crate::{core::render, windows::EditorWindow};
use glib::{Object, subclass::types::ObjectSubclassIsExt};

mod imp {
    use std::cell::OnceCell;
    use std::cell::RefCell;
    use std::fs::File;
    use std::os::fd::AsFd;
    use std::path::PathBuf;

    use adw::prelude::ComboRowExt;
    use adw::subclass::prelude::*;
    use glib::subclass::InitializingObject;
    use glib::subclass::types::ObjectSubclassIsExt;
    use gtk::CompositeTemplate;
    use gtk::glib;
    use gtk::prelude::*;

    use crate::runtime;
    use crate::{core::render, windows::EditorWindow};

    #[derive(CompositeTemplate, Default)]
    #[template(resource = "/fr/alexpiquard/ferricast/ui/render.ui")]
    pub struct RenderWindow {
        #[template_child]
        pub format_input: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub start_time_input: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub end_time_input: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub content_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub action_bar_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub render_action_bar_end_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub formats_list: TemplateChild<gtk::StringList>,
        #[template_child]
        pub progress_bar: TemplateChild<gtk::ProgressBar>,
        pub editor: OnceCell<EditorWindow>,
        pub output_path: RefCell<Option<PathBuf>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RenderWindow {
        const NAME: &'static str = "RenderWindow";
        type Type = super::RenderWindow;
        type ParentType = adw::Window;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for RenderWindow {}

    impl WidgetImpl for RenderWindow {}

    impl WindowImpl for RenderWindow {
        fn close_request(&self) -> glib::Propagation {
            self.editor()
                .imp()
                .save_render_settings(self.render_settings());
            glib::Propagation::Proceed
        }
    }

    impl AdwWindowImpl for RenderWindow {}

    #[gtk::template_callbacks]
    impl RenderWindow {
        #[template_callback]
        fn handle_render_clicked(&self) {
            let this = self.obj().clone();
            self.ask_render_output(move |path| {
                this.imp().show_stackpage("render");
                this.imp().render(path.clone());
                this.imp().render_action_bar_end_box.set_visible(false);
                this.imp().output_path.replace(Some(path));
            });
        }

        #[template_callback]
        fn handle_render_cancel_clicked(&self) {
            self.show_stackpage("settings");
            self.progress_bar.set_fraction(0.0);
        }

        #[template_callback]
        fn handle_open_clicked(&self) {
            let Some(output_directory_path) = self.output_path() else {
                return;
            };
            let Ok(directory) = File::open(output_directory_path) else {
                tracing::warn!("failed to open output directory");
                return;
            };

            runtime().spawn(async move {
                if let Err(e) = ashpd::desktop::open_uri::OpenDirectoryRequest::default()
                    .send(&directory.as_fd())
                    .await
                {
                    tracing::warn!("failed to open output directory: {:?}", e);
                }
            });
        }

        #[template_callback]
        fn handle_play_clicked(&self) {
            let Some(output_file) = self.output_path() else {
                return;
            };
            let Ok(file) = File::open(output_file) else {
                tracing::warn!("failed to open output file");
                return;
            };

            runtime().spawn(async move {
                if let Err(e) = ashpd::desktop::open_uri::OpenFileRequest::default()
                    .ask(true)
                    .send_file(&file.as_fd())
                    .await
                {
                    tracing::warn!("failed to open output file: {:?}", e);
                }
            });
        }

        #[template_callback]
        fn handle_settings_cancel_clicked(&self) {
            self.obj().close();
        }
    }

    impl RenderWindow {
        pub fn setup(&self, editor: &EditorWindow, settings: &render::RenderSettings) {
            self.editor
                .set(editor.clone())
                .expect("failed to set recording_file");

            if let Some(duration) = self.video_duration() {
                self.end_time_input
                    .adjustment()
                    .set_upper(duration.seconds_f64());
                self.end_time_input
                    .set_value(settings.end_sec().unwrap_or(duration.seconds_f64()));
            }

            render::formats()
                .iter()
                .for_each(|format_name| self.formats_list.append(format_name));
            self.format_input.set_selected(settings.format_position());
            self.start_time_input.set_value(settings.start_sec());
        }

        fn show_stackpage(&self, name: &str) {
            self.content_stack.set_visible_child_name(name);
            self.action_bar_stack.set_visible_child_name(name);
        }

        fn editor(&self) -> &EditorWindow {
            self.editor.get().unwrap()
        }

        fn video_duration(&self) -> Option<gst::ClockTime> {
            self.editor().imp().video().duration()
        }

        fn output_path(&self) -> Option<PathBuf> {
            self.output_path.borrow().clone()
        }

        fn render(&self, output_path: PathBuf) {
            let settings = self.render_settings();
            let recording_file = self.editor().imp().video().recording_file().to_path_buf();
            let zoom_effects = self.editor().imp().video().zoom_effects();

            let (progress_sender, progress_receiver) = async_channel::bounded::<f64>(1);

            runtime().spawn(async move {
                let progress_sender = progress_sender;
                render::render(
                    settings,
                    recording_file,
                    output_path,
                    zoom_effects,
                    move |p| {
                        progress_sender.try_send(p).ok();
                    },
                )
            });

            glib::spawn_future_local(glib::clone!(
                #[weak(rename_to=this)]
                self,
                async move {
                    while let Ok(progress) = progress_receiver.recv().await {
                        this.progress_bar.set_fraction(progress);
                        if progress == 1.0 {
                            this.after_render();
                            break;
                        }
                    }
                }
            ));
        }

        fn after_render(&self) {
            self.render_action_bar_end_box.set_visible(true);
        }

        fn render_settings(&self) -> render::RenderSettings {
            let format = self.format_input.get().selected();
            let start_sec = self.start_time_input.get().value();
            let end_sec = self.end_time_input.get().value();
            let video_duration = self.video_duration();
            let opt_duration_sec = if let Some(video_duration) = video_duration
                && video_duration.seconds_f64() == end_sec
            {
                None
            } else {
                Some(end_sec)
            };
            render::RenderSettings::new(format, start_sec, opt_duration_sec)
        }

        fn ask_render_output(&self, then: impl FnOnce(PathBuf) + 'static) {
            let dialog = gtk::FileDialog::builder()
                .title("Export video")
                .accept_label("Export")
                .initial_name(format!(
                    "export.{}",
                    self.render_settings().format().extension()
                ))
                .modal(true)
                .build();

            let then = Box::new(then);

            let window = self.obj().clone();
            dialog.save(
                Some(&window),
                None::<&gtk::gio::Cancellable>,
                move |result: Result<gtk::gio::File, gst::glib::Error>| {
                    if let Ok(file) = result
                        && let Some(path) = file.path()
                    {
                        then(path);
                    }
                },
            );
        }
    }
}

glib::wrapper! {
    pub struct RenderWindow(ObjectSubclass<imp::RenderWindow>)
        @extends adw::Window, gtk::Window, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;

}

impl RenderWindow {
    pub fn new(editor: &EditorWindow, settings: &render::RenderSettings) -> Self {
        let window: Self = Object::builder().build();
        window.imp().setup(editor, settings);
        window
    }
}
