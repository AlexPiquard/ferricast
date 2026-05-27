mod application;
mod config;
mod core;
mod widgets;
mod windows;

use std::sync::OnceLock;

use gettextrs::LocaleCategory;
use gtk::{gio, glib};
use tokio::runtime::Runtime;

use crate::config::{APP_NAME, GETTEXT_PACKAGE, LOCALEDIR, RESOURCES_FILE};

use self::application::Application;

fn main() -> glib::ExitCode {
    // initialize logger
    tracing_subscriber::fmt::init();

    // initialize gstreamer
    gst::init().expect("failed to initialize GStreamer");
    ges::init().expect("failed to init GES");

    // prepare i18n
    gettextrs::setlocale(LocaleCategory::LcAll, "");
    gettextrs::bindtextdomain(*GETTEXT_PACKAGE, *LOCALEDIR)
        .expect("unable to bind the text domain");
    gettextrs::textdomain(*GETTEXT_PACKAGE).expect("unable to switch to the text domain");

    glib::set_application_name(*APP_NAME);

    let res = gio::Resource::load(*RESOURCES_FILE).expect("could not load gresource file");
    gio::resources_register(&res);

    let app = Application::default();
    app.run()
}
#[cfg(test)]
mod tests {
    #[allow(unused)]
    use super::*;

    // TODO: tests
}

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| Runtime::new().expect("failed to setup runtime"))
}
