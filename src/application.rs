use tracing::info;

use gettextrs::gettext;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, gio, glib};

use crate::config::{APP_ID, APP_NAME, PATH_ID, PKGDATADIR, PROFILE, VERSION};
use crate::windows::MainWindow;

mod imp {

    use crate::config::{APP_ID, PATH_ID, VERSION};

    use super::*;
    use adw::{prelude::AdwDialogExt, subclass::prelude::AdwApplicationImpl};
    use glib::WeakRef;
    use gtk::gio;
    use std::cell::OnceCell;

    #[derive(Debug, Default)]
    pub struct Application {
        pub window: OnceCell<WeakRef<MainWindow>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Application {
        const NAME: &'static str = "Application";
        type Type = super::Application;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for Application {}

    impl ApplicationImpl for Application {
        fn activate(&self) {
            self.parent_activate();
            let app = self.obj();

            if let Some(window) = self.window.get() {
                let window = window.upgrade().unwrap();
                window.present();
                return;
            }

            let window = MainWindow::new(&app);
            self.window
                .set(window.downgrade())
                .expect("Window already set.");

            self.main_window().present();
        }

        fn startup(&self) {
            self.parent_startup();

            // set icons for shell
            gtk::Window::set_default_icon_name(*APP_ID);

            self.setup_css();
            self.setup_gactions();
            self.setup_accels();
        }
    }

    impl GtkApplicationImpl for Application {}
    impl AdwApplicationImpl for Application {}

    impl Application {
        fn main_window(&self) -> MainWindow {
            self.window.get().unwrap().upgrade().unwrap()
        }

        fn setup_gactions(&self) {
            let action_quit = gio::ActionEntry::builder("quit")
                .activate(move |app: &crate::Application, _, _| {
                    // this is needed to trigger the delete event and saving the window state
                    app.active_window().inspect(|w| w.close());
                    app.quit();
                })
                .build();

            // About
            let action_about = gio::ActionEntry::builder("about")
                .activate(|app: &crate::Application, _, _| {
                    app.imp().show_about_dialog();
                })
                .build();
            self.obj().add_action_entries([action_quit, action_about]);
        }

        // sets up keyboard shortcuts
        fn setup_accels(&self) {
            self.obj()
                .set_accels_for_action("app.quit", &["<Control>q"]);
            self.obj()
                .set_accels_for_action("window.close", &["<Control>w"]);
        }

        fn setup_css(&self) {
            let provider = gtk::CssProvider::new();
            provider.load_from_resource(format!("{}style.css", *PATH_ID).as_ref());
            if let Some(display) = gdk::Display::default() {
                gtk::style_context_add_provider_for_display(
                    &display,
                    &provider,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
            }
        }

        fn authors() -> Vec<&'static str> {
            env!("CARGO_PKG_AUTHORS").split(":").collect()
        }

        fn show_about_dialog(&self) {
            let dialog = adw::AboutDialog::from_appdata(
                &format!("{}/metainfo.xml", *PATH_ID),
                Some(*VERSION),
            );

            dialog.set_version(*VERSION);
            dialog.set_developers(&Self::authors());

            dialog.set_translator_credits(&gettext("translator-credits"));

            dialog.present(self.obj().active_window().as_ref());
        }
    }
}

glib::wrapper! {
    pub struct Application(ObjectSubclass<imp::Application>)
        @extends gio::Application, gtk::Application, adw::Application,
        @implements gio::ActionMap, gio::ActionGroup;
}

impl Application {
    pub fn run(&self) -> glib::ExitCode {
        info!("{} ({})", *APP_NAME, *APP_ID);
        info!("Version: {} ({})", *VERSION, *PROFILE);
        info!("Datadir: {}", *PKGDATADIR);

        ApplicationExtManual::run(self)
    }
}

impl Default for Application {
    fn default() -> Self {
        glib::Object::builder()
            .property("application-id", *APP_ID)
            .property("resource-base-path", *PATH_ID)
            .build()
    }
}
