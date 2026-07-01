use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const CURSOR_INTERVAL_MS: u128 = 16;

pub struct CursorFile {
    writer: BufWriter<File>,
    last_write_pts: u128,
}

impl CursorFile {
    pub fn new(path: &Path) -> Self {
        let path = &path.with_extension("curs");
        let path_str = path.to_str().unwrap();
        let file = File::create(path_str).expect("failed to create cursor file");
        Self {
            writer: BufWriter::new(file),
            last_write_pts: 0,
        }
    }

    pub fn write(&mut self, pts: u64, x: i32, y: i32, cursor_type_hash: Option<u64>) {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        if now_ms - self.last_write_pts < CURSOR_INTERVAL_MS {
            return;
        }
        self.last_write_pts = now_ms;

        let _ = self.writer.write(&pts.to_le_bytes());
        let _ = self.writer.write(&x.to_le_bytes());
        let _ = self.writer.write(&y.to_le_bytes());

        let hash_bytes = cursor_type_hash.unwrap_or(0);
        let _ = self.writer.write(&hash_bytes.to_le_bytes());
    }

    fn flush(&mut self) {
        let _ = self.writer.flush();
    }

    pub fn img_path(recording_file: &Path, hash: u64) -> String {
        format!(
            "{}_{}.png",
            recording_file.with_extension("").to_str().unwrap(),
            hash
        )
    }
}

impl Drop for CursorFile {
    fn drop(&mut self) {
        self.flush();
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CursorEntry {
    pub pts: f64,
    // pub initial_x: f64,
    // pub initial_y: f64,
    pub x: f64,
    pub y: f64,
    pub cursor_type_hash: Option<u64>,
}

impl CursorEntry {
    pub fn new(pts: u64, x: i32, y: i32, cursor_type_hash: Option<u64>) -> Self {
        Self {
            pts: pts as f64,
            x: x as f64,
            y: y as f64,
            // initial_x: x as f64,
            // initial_y: y as f64,
            cursor_type_hash,
        }
    }
    pub fn from_f64(pts: f64, x: f64, y: f64, cursor_type_hash: Option<u64>) -> Self {
        Self {
            pts,
            x,
            y,
            // initial_x: x,
            // initial_y: y,
            cursor_type_hash,
        }
    }
}
