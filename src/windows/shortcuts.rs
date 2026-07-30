use adw::prelude::*;

pub fn setup_shortcuts(window: &adw::ApplicationWindow) {
    let builder = gtk::Builder::from_resource("/fr/alexpiquard/ferricast/gtk/help-overlay.ui");
    if let Some(dialog) = builder.object::<adw::Dialog>("help_overlay") {
        let obj = window.downgrade();
        let action = gtk::gio::SimpleAction::new("show-help-overlay", None);
        action.connect_activate(move |_, _| {
            if let Some(window) = obj.upgrade() {
                dialog.present(Some(&window));
            }
        });
        window.add_action(&action);
    } else {
        tracing::info!("not found!!");
    }
}
