//! Mini floating window with smart screen placement.
//!
//! A hotkey-summoned, ~1/10-screen companion window (ChatGPT-mini style). Rather
//! than always landing dead-center over the user's work, it screenshots the
//! display, finds the *coldest* region (lowest visual activity = empty desktop,
//! blank panel, whitespace), and drops itself into that gap.
//!
//! The placement detector is a local edge-activity map + summed-area table —
//! precise pixel coords, instant, nothing leaves the machine. (A vision-model
//! refinement lives in scripts/mini-placement.py for the semantic-cold case.)

use std::path::PathBuf;
use std::process::Command;

use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder,
};

const MINI_LABEL: &str = "mini";

// Detector tuning. Mirrors scripts/mini-placement.py so behavior matches the
// validated prototype.
const GRID_W: u32 = 192; // downscale width for the activity grid
const MENUBAR_FRAC: f32 = 0.035; // reserve the top strip (macOS menubar/notch)
const EDGE_MARGIN: f32 = 0.012; // keep a gap from screen edges
const AREA_FRAC: f32 = 0.10; // target window area as a fraction of the screen
const RATIO: f32 = 0.62; // window width / height (portrait, ChatGPT-mini-ish)

#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Capture the main display to a temp PNG and load it. macOS-only (screencapture);
/// other platforms fall back to centered placement.
fn capture_screen() -> Option<image::RgbaImage> {
    // Private, randomly-named temp file: created O_EXCL (no symlink/TOCTOU
    // window on a predictable path) and auto-removed when `tmp` drops — even if
    // we early-return or panic. screencapture overwrites the empty file in place.
    let tmp = tempfile::Builder::new()
        .prefix("puffer-mini-shot-")
        .suffix(".png")
        .tempfile()
        .ok()?;
    let ok = Command::new("/usr/sbin/screencapture")
        .args(["-x", "-m", tmp.path().to_str()?])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return None;
    }
    image::open(tmp.path()).ok().map(|i| i.to_rgba8())
}

/// Find the coldest target-sized rectangle in screen pixels.
fn coldest_placement(img: &image::RgbaImage) -> Placement {
    let (w, h) = img.dimensions();
    // A degenerate screenshot (empty / absurd aspect) would make the grid too
    // small for the 2-cell window + margins and panic the clamps below. Fall
    // back to a simple centered ~1/10 rect in that case.
    if w < 8 || h < 8 {
        let ww = ((w as f32 * 0.24) as u32).max(1);
        let hh = ((ww as f32 / RATIO) as u32).max(1);
        return Placement {
            x: (w.saturating_sub(ww) / 2) as i32,
            y: (h.saturating_sub(hh) / 2) as i32,
            w: ww,
            h: hh,
        };
    }
    let gw = GRID_W.min(w.max(1)).max(4);
    let gh = ((((gw as f32) * h as f32 / w as f32).round() as u32).max(1)).max(4);

    // Downscale, then build the per-cell edge-activity map (|dx|+|dy| of luma).
    let small = image::imageops::resize(img, gw, gh, image::imageops::FilterType::Triangle);
    let lum = |x: u32, y: u32| -> f32 {
        let p = small.get_pixel(x, y).0;
        0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
    };
    let mut act = vec![0f32; (gw * gh) as usize];
    for y in 0..gh {
        for x in 0..gw {
            let l = lum(x, y);
            let dx = if x > 0 { (l - lum(x - 1, y)).abs() } else { 0.0 };
            let dy = if y > 0 { (l - lum(x, y - 1)).abs() } else { 0.0 };
            act[(y * gw + x) as usize] = dx + dy;
        }
    }

    // Target window size in grid cells.
    let target_area = AREA_FRAC * gw as f32 * gh as f32;
    let tw = ((target_area * RATIO).sqrt().round() as u32).clamp(2, gw);
    let th = ((tw as f32 / RATIO).round() as u32).clamp(2, gh);

    // Summed-area table for O(1) rectangle sums (padded by 1).
    let sw = (gw + 1) as usize;
    let mut sat = vec![0f64; sw * (gh as usize + 1)];
    for y in 0..gh {
        for x in 0..gw {
            let v = act[(y * gw + x) as usize] as f64;
            let i = (y as usize + 1) * sw + (x as usize + 1);
            sat[i] = v + sat[i - 1] + sat[i - sw] - sat[i - sw - 1];
        }
    }
    let rect_sum = |i: u32, j: u32| -> f64 {
        let a = |yy: u32, xx: u32| sat[(yy as usize) * sw + xx as usize];
        a(i + th, j + tw) - a(i, j + tw) - a(i + th, j) + a(i, j)
    };

    // Search window, avoiding the menubar strip and edge margins.
    let top_lo = (MENUBAR_FRAC * gh as f32).round() as u32;
    let mi = (EDGE_MARGIN * gh as f32).round() as u32;
    let mj = (EDGE_MARGIN * gw as f32).round() as u32;
    let (mut i_lo, mut i_hi) = (top_lo + mi, gh.saturating_sub(th + mi));
    let (mut j_lo, mut j_hi) = (mj, gw.saturating_sub(tw + mj));
    if i_hi < i_lo {
        i_lo = 0;
        i_hi = gh.saturating_sub(th);
    }
    if j_hi < j_lo {
        j_lo = 0;
        j_hi = gw.saturating_sub(tw);
    }

    let mut best: Option<(f64, u32, u32)> = None;
    for i in i_lo..=i_hi {
        for j in j_lo..=j_hi {
            let s = rect_sum(i, j);
            // Tie-break toward screen edges / lower regions (less likely the
            // user's active focus).
            let edge_pull = j.min(gw.saturating_sub(tw).saturating_sub(j)) as f64
                + 0.5 * (gh.saturating_sub(th).saturating_sub(i)) as f64;
            let score = s + 0.15 * edge_pull;
            if best.map_or(true, |b| score < b.0) {
                best = Some((score, i, j));
            }
        }
    }
    let (_, gi, gj) = best.unwrap_or((0.0, gh / 3, gw / 3));

    Placement {
        x: (gj as f32 / gw as f32 * w as f32).round() as i32,
        y: (gi as f32 / gh as f32 * h as f32).round() as i32,
        w: (tw as f32 / gw as f32 * w as f32).round() as u32,
        h: (th as f32 / gh as f32 * h as f32).round() as u32,
    }
}

/// Centered ~1/10-screen fallback when capture is unavailable.
fn centered_placement(app: &AppHandle) -> Placement {
    let (sw, sh) = app
        .primary_monitor()
        .ok()
        .flatten()
        .map(|m| {
            let s = m.size();
            (s.width, s.height)
        })
        .unwrap_or((1440, 900));
    let w = (sw as f32 * 0.24) as u32;
    let h = (w as f32 / RATIO) as u32;
    Placement {
        x: ((sw.saturating_sub(w)) / 2) as i32,
        y: ((sh.saturating_sub(h)) * 2 / 3) as i32,
        w,
        h,
    }
}

/// Compute placement: cold-region detection if a screenshot is available,
/// otherwise a centered fallback. Both detectors work in display-local
/// coordinates (0,0 = top-left of the primary display); we add the primary
/// monitor's global origin so the window lands correctly in multi-monitor
/// layouts where the primary isn't at the global origin.
fn compute_placement(app: &AppHandle) -> Placement {
    let mut placement = match capture_screen() {
        Some(img) => coldest_placement(&img),
        None => centered_placement(app),
    };
    if let Some(monitor) = app.primary_monitor().ok().flatten() {
        let origin = monitor.position();
        placement.x += origin.x;
        placement.y += origin.y;
    }
    placement
}

fn frontend_url() -> WebviewUrl {
    // The mini window renders a compact view (main.ts routes on the hash).
    WebviewUrl::App(PathBuf::from("index.html#mini"))
}

/// Toggle the mini window: create-or-show at the computed cold spot; hide if
/// already visible (so the same hotkey dismisses it).
pub fn toggle_mini_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(MINI_LABEL) {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
            return;
        }
        let p = compute_placement(app);
        let _ = win.set_size(PhysicalSize::new(p.w, p.h));
        let _ = win.set_position(PhysicalPosition::new(p.x, p.y));
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }

    let p = compute_placement(app);
    match WebviewWindowBuilder::new(app, MINI_LABEL, frontend_url())
        .title("puffer")
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(true)
        .inner_size(p.w as f64, p.h as f64)
        .build()
    {
        Ok(win) => {
            // Builder sizes/positions in logical px; re-apply in physical px so
            // retina scaling lands the window exactly on the detected region.
            let _ = win.set_size(PhysicalSize::new(p.w, p.h));
            let _ = win.set_position(PhysicalPosition::new(p.x, p.y));
            let _ = win.set_focus();
        }
        Err(err) => {
            eprintln!("mini window: failed to build: {err}");
        }
    }
}

/// Tauri command so the frontend (or a menu) can summon the mini window too.
#[tauri::command]
pub fn summon_mini_window(app: AppHandle) -> Result<(), String> {
    toggle_mini_window(&app);
    Ok(())
}
