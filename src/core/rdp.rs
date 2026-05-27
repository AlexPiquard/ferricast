#[derive(Debug, Clone, Copy)]
pub struct CursorEntry {
    pts: f64,
    initial_x: f64,
    initial_y: f64,
    x: f64,
    y: f64,
}

impl CursorEntry {
    pub fn new(pts: u64, x: i32, y: i32) -> Self {
        Self {
            pts: pts as f64,
            x: x as f64,
            y: y as f64,
            initial_x: x as f64,
            initial_y: y as f64,
        }
    }
    pub fn from_f64(pts: f64, x: f64, y: f64) -> Self {
        Self {
            pts,
            x,
            y,
            initial_x: x,
            initial_y: y,
        }
    }
}

/// adjust cursor entries locations to match an image
pub fn adjust_cursor_for_image(points: &mut Vec<CursorEntry>, offset_x: f64, offset_y: f64) {
    for point in points.iter_mut() {
        point.x = point.initial_x - offset_x;
        point.y = point.initial_y - offset_y;
    }
}

fn perpendicular_distance(p: CursorEntry, a: CursorEntry, b: CursorEntry) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let dpts = b.pts - a.pts;

    let mag_sq = dx * dx + dy * dy + dpts * dpts;
    if mag_sq == 0.0 {
        return ((p.x - a.x).powi(2) + (p.y - a.y).powi(2) + (p.pts - a.pts).powi(2)).sqrt();
    }

    let u = ((p.x - a.x) * dx + (p.y - a.y) * dy + (p.pts - a.pts) * dpts) / mag_sq;

    let closest_point = CursorEntry::from_f64(a.pts + u * dpts, a.x + u * dx, a.y + u * dy);

    ((p.x - closest_point.x).powi(2)
        + (p.y - closest_point.y).powi(2)
        + (p.pts - closest_point.pts).powi(2))
    .sqrt()
}

pub fn ramer_douglas_peucker(
    points: &[CursorEntry],
    from: usize,
    to: usize,
    epsilon: f64,
) -> Vec<usize> {
    if points.len() < 3 {
        return (from..=to).collect();
    }

    let mut dmax = 0.0;
    let mut index = from;

    for i in (from + 1)..(to - 1) {
        let d = perpendicular_distance(points[i], points[from], points[to]);
        if d > dmax {
            index = i;
            dmax = d;
        }
    }

    if dmax > epsilon {
        let mut left = ramer_douglas_peucker(points, from, index, epsilon);
        let mut right = ramer_douglas_peucker(points, index, to, epsilon);
        left.pop();
        left.append(&mut right);
        left
    } else {
        vec![from, to]
    }
}

pub fn get_position_at_time(points: &[CursorEntry], current_time: f64) -> (f64, f64) {
    // find the index of the segment corresponding to the current time
    let idx = points
        .windows(2)
        .position(|w| current_time >= w[0].pts && current_time < w[1].pts)
        .unwrap_or(0);

    // get the 4 control points (with edge protection)
    let p0 = points[idx.saturating_sub(1)];
    let p1 = points[idx];
    let p2 = points[(idx + 1).min(points.len() - 1)];
    let p3 = points[(idx + 2).min(points.len() - 1)];

    // calculate 'u' (normalized progress between p1.t and p2.t)
    let u = (current_time - p1.pts) / (p2.pts - p1.pts);

    // standard Catmull-Rom interpolation
    let x = catmull_rom_1d(p0.x, p1.x, p2.x, p3.x, u);
    let y = catmull_rom_1d(p0.y, p1.y, p2.y, p3.y, u);

    (x, y)
}

fn catmull_rom_1d(p0: f64, p1: f64, p2: f64, p3: f64, t: f64) -> f64 {
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t * t * t)
}
