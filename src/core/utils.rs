use std::path::PathBuf;

use adw::prelude::*;

/// shows an error dialog that closes the parent window when dismissed.
pub fn show_error_dialog_and_close(parent: &impl IsA<gtk::Window>, heading: &str, body: &str) {
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(body)
        .build();
    dialog.add_response("ok", "OK");
    let window = parent.clone();
    dialog.connect_response(None, move |_dialog, _response| {
        window.close();
    });
    let parent_upcast: &gtk::Window = parent.upcast_ref();
    dialog.present(Some(parent_upcast));
}

pub fn tmp_dir(folder: Option<String>) -> PathBuf {
    let mut tmp_dir = std::env::temp_dir().join("ferricast");

    if let Some(folder) = folder {
        tmp_dir.push(folder);
    }

    std::fs::create_dir_all(&tmp_dir).ok();
    tmp_dir
}
