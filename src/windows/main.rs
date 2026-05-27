use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use crate::application::Application;

mod imp {
    use std::path::{Path, PathBuf};

    use adw::subclass::prelude::AdwApplicationWindowImpl;

    use crate::{config::PROFILE, core::screencast, runtime, windows};

    use super::*;

    #[derive(Debug, gtk::CompositeTemplate, Default)]
    #[template(resource = "/fr/alexpiquard/ferricast/ui/window.ui")]
    pub struct MainWindow {
        #[template_child]
        pub headerbar: TemplateChild<adw::HeaderBar>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MainWindow {
        const NAME: &'static str = "MainWindow";
        type Type = super::MainWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MainWindow {
        fn constructed(&self) {
            self.parent_constructed();

            if *PROFILE == "Devel" {
                self.obj().add_css_class("devel");
            }
        }
    }

    impl WidgetImpl for MainWindow {}
    impl WindowImpl for MainWindow {}

    impl ApplicationWindowImpl for MainWindow {}
    impl AdwApplicationWindowImpl for MainWindow {}

    #[gtk::template_callbacks]
    impl MainWindow {
        #[template_callback]
        fn handle_record_clicked(&self) {
            self.screencast()
        }

        #[template_callback]
        fn handle_open_clicked(&self) {
            let obj = self.obj().clone();
            self.select_existing_video(move |path| {
                if let Some(path) = path {
                    obj.imp().show_player_window(&path);
                }
            });
        }
    }

    enum ScreencastChannelEvent {
        Prepared,
        Finished(PathBuf),
    }

    impl MainWindow {
        fn app(&self) -> gtk::Application {
            self.obj().application().expect("failed to get application")
        }

        fn screencast(&self) {
            let (sender, receiver) = async_channel::bounded::<ScreencastChannelEvent>(1);

            runtime().spawn(async move {
                if let Ok((pipewire_node_id, fd, temp_filename)) =
                    screencast::prepare_screencast().await
                {
                    sender
                        .send(ScreencastChannelEvent::Prepared)
                        .await
                        .expect("failed to use screencast channel when prepared");
                    if let Err(e) =
                        screencast::start_screencast(pipewire_node_id, fd, temp_filename.clone())
                    {
                        eprintln!("Screencast error: {}", e);
                    } else {
                        sender
                            .send(ScreencastChannelEvent::Finished(temp_filename))
                            .await
                            .expect("failed to use screencast channel when finished");
                    }
                }
            });

            glib::spawn_future_local(glib::clone!(
                #[weak(rename_to=app)]
                self.app(),
                #[weak(rename_to=obj)]
                self.obj(),
                async move {
                    while let Ok(response) = receiver.recv().await {
                        match response {
                            ScreencastChannelEvent::Prepared => {
                                // hide windows when recording
                                for window in app.windows() {
                                    window.set_visible(false);
                                }
                            }
                            ScreencastChannelEvent::Finished(temp_filename) => {
                                // show video in editor when screencast ends
                                obj.imp().show_player_window(&temp_filename);
                            }
                        }
                    }
                }
            ));
        }

        fn select_existing_video(&self, then: impl FnOnce(Option<PathBuf>) + 'static) {
            let dialog = gtk::FileDialog::builder()
                .title("Select video")
                .accept_label("Select")
                .modal(true)
                .build();

            let then = Box::new(then);

            dialog.open(
                Some(self.obj().as_ref()),
                None::<&gtk::gio::Cancellable>,
                move |result: Result<gtk::gio::File, gst::glib::Error>| {
                    if let Ok(file) = result {
                        then(file.path());
                    } else {
                        then(None);
                    }
                },
            );
        }

        fn clear_windows(&self) {
            let app = self.app();
            if let Some(window) = app.active_window() {
                window.close();
            }
        }

        fn show_player_window(&self, recording_file: &Path) {
            let app = self.app();

            self.clear_windows();

            let window = windows::EditorWindow::new(&app, recording_file.to_path_buf());
            window.present();
        }
    }
}

glib::wrapper! {
    pub struct MainWindow(ObjectSubclass<imp::MainWindow>)
        @extends gtk::Widget, adw::Window, gtk::Window, adw::ApplicationWindow, gtk::ApplicationWindow,
        @implements gio::ActionMap, gio::ActionGroup, gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl MainWindow {
    pub fn new(app: &Application) -> Self {
        glib::Object::builder().property("application", app).build()
    }
}
