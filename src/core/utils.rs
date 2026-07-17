use std::path::{Path, PathBuf};

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

pub fn resolve_portal_path(portal_path: &Path) -> PathBuf {
    if let Ok(Some(xattr_value)) = xattr::get(portal_path, "user.document-portal.host-path") {
        if let Ok(real_path_str) = String::from_utf8(xattr_value) {
            let clean_path = real_path_str.trim_end_matches('\0');
            return PathBuf::from(clean_path);
        }
    }
    portal_path.to_path_buf()
}
