//! Draws the widget into a premultiplied BGRA pixel buffer.
//!
//! GDI can't draw text with alpha, so text is drawn white-on-black into a mask
//! and its brightness becomes coverage, then tinted. The Claude mark is drawn
//! analytically (supersampled capsules), so it stays crisp at any DPI.

use crate::http::wide;
use windows_sys::Win32::Foundation::{RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::*;

#[derive(Clone, Copy, PartialEq)]
pub struct Rgba(pub f32, pub f32, pub f32, pub f32);

pub struct Palette {
    pub label: Rgba,
    pub value: Rgba,
    pub warn: Rgba,
    pub crit: Rgba,
    pub divider: Rgba,
    pub hover: Rgba,
}

pub fn palette(dark: bool) -> Palette {
    let c = |hex: u32, a: f32| {
        Rgba(
            ((hex >> 16) & 255) as f32 / 255.0,
            ((hex >> 8) & 255) as f32 / 255.0,
            (hex & 255) as f32 / 255.0,
            a,
        )
    };
    if dark {
        Palette {
            label: c(0xFFFFFF, 0.62),
            value: c(0xFFFFFF, 1.0),
            warn: c(0xF5A35C, 1.0),
            crit: c(0xFF7B72, 1.0),
            divider: c(0xFFFFFF, 0.22),
            hover: c(0xFFFFFF, 0.07),
        }
    } else {
        Palette {
            label: c(0x000000, 0.52),
            value: c(0x1B1B1B, 1.0),
            warn: c(0xB4500F, 1.0),
            crit: c(0xC42B1C, 1.0),
            divider: c(0x000000, 0.18),
            hover: c(0xFFFFFF, 0.55),
        }
    }
}

pub const CLAUDE_ORANGE: Rgba = Rgba(0.851, 0.467, 0.341, 1.0); // #D97757

/// One account's numbers.
pub struct Row {
    pub five: Option<f64>,
    pub week: Option<f64>,
    pub stale: bool,
}

/// Most accounts that fit in one column.
pub const MAX_ROWS_PER_COLUMN: usize = 3;
/// The "Default" font size; other sizes scale relative to it.
pub const DEFAULT_FONT_SIZE: u32 = 14;

pub struct Model {
    /// One per account, laid out top to bottom, then left to right.
    pub rows: Vec<Row>,
    pub hover: bool,
    pub dark: bool,
    /// Show what's left (100 - used) instead of what's used.
    pub show_left: bool,
    /// Text size in pixels at 100% scaling.
    pub font_size: u32,
    /// Leave out the Claude mark.
    pub hide_icon: bool,
    /// Leave out the `5h` / `W` labels, showing only the numbers.
    pub hide_labels: bool,
    /// Accounts stacked per column (1 to MAX_ROWS_PER_COLUMN); more start a new column.
    pub rows_per_col: usize,
}

pub struct Frame {
    pub w: i32,
    pub h: i32,
    pub px: Vec<[u8; 4]>, // premultiplied B, G, R, A
}

fn value_text(v: Option<f64>, show_left: bool) -> String {
    match v {
        Some(p) => {
            let used = p.round().clamp(0.0, 100.0) as i64;
            format!("{}%", if show_left { 100 - used } else { used })
        }
        None => "–".into(),
    }
}

fn value_color(v: Option<f64>, pal: &Palette) -> Rgba {
    match v {
        Some(p) if p >= 95.0 => pal.crit,
        Some(p) if p >= 80.0 => pal.warn,
        _ => pal.value,
    }
}

unsafe fn make_font(px: i32, faces: &[&str]) -> HFONT {
    for (i, face) in faces.iter().enumerate() {
        let f = CreateFontW(
            -px, 0, 0, 0, FW_NORMAL as i32, 0, 0, 0,
            DEFAULT_CHARSET as u32, OUT_TT_PRECIS as u32, CLIP_DEFAULT_PRECIS as u32,
            ANTIALIASED_QUALITY as u32, (DEFAULT_PITCH | FF_SWISS) as u32,
            wide(face).as_ptr(),
        );
        if i + 1 == faces.len() || font_is(f, face) {
            return f;
        }
        DeleteObject(f);
    }
    unreachable!()
}

/// GDI silently substitutes missing fonts; check we got the face we asked for.
unsafe fn font_is(f: HFONT, face: &str) -> bool {
    let dc = CreateCompatibleDC(std::ptr::null_mut());
    let old = SelectObject(dc, f);
    let mut buf = [0u16; 64];
    let n = GetTextFaceW(dc, buf.len() as i32, buf.as_mut_ptr());
    SelectObject(dc, old);
    DeleteDC(dc);
    let got = String::from_utf16_lossy(&buf[..n.max(1) as usize - 1]);
    got.eq_ignore_ascii_case(face)
}

struct Seg {
    text: Vec<u16>,
    bold: bool,
    x: i32,
    w: i32,
    /// Vertical band of the account row this text belongs to.
    y0: i32,
    y1: i32,
    color: Rgba,
}

struct Divider {
    x: i32,
    y0: i32,
    y1: i32,
}

/// Renders the widget for a taskbar of height `h` at `scale` (DPI / 96).
pub fn render(m: &Model, h: i32, scale: f32) -> Frame {
    let pal = palette(m.dark);
    let s = |v: f32| (v * scale).round() as i32;
    let empty = [Row { five: None, week: None, stale: false }];
    let rows: &[Row] = if m.rows.is_empty() { &empty } else { &m.rows };
    let rows_per_col = m.rows_per_col.clamp(1, MAX_ROWS_PER_COLUMN);
    let per_col = rows.len().min(rows_per_col);
    // One account uses the full height like before; stacked accounts share
    // it, with a small margin, and the text shrinks to fit.
    let (band_top, row_h) = if per_col == 1 {
        (0.0, h as f32)
    } else {
        let margin = s(1.0) as f32;
        (margin, (h as f32 - 2.0 * margin) / per_col as f32)
    };
    let row_band = |r: usize| -> (i32, i32) {
        let y0 = (band_top + r as f32 * row_h).round() as i32;
        let y1 = (band_top + (r + 1) as f32 * row_h).round() as i32;
        (y0, y1.min(h))
    };
    unsafe {
        let mut font_px = s(m.font_size as f32);
        if per_col > 1 {
            // Stacked rows: the Default size fills most of a row, and the
            // other sizes scale from there, never taller than the row or
            // larger than they'd be on a single row.
            let fit = row_h * 0.8 * m.font_size as f32 / DEFAULT_FONT_SIZE as f32;
            font_px = font_px.min(fit.min(row_h).round() as i32).max(1);
        }
        let f_label = make_font(font_px, &["Segoe UI Variable Text", "Segoe UI"]);
        let f_value = make_font(font_px, &["Segoe UI Variable Text Semibold", "Segoe UI Semibold"]);

        let dc = CreateCompatibleDC(std::ptr::null_mut());
        let measure = |t: &[u16], f: HFONT| -> i32 {
            SelectObject(dc, f);
            let mut sz = SIZE { cx: 0, cy: 0 };
            GetTextExtentPoint32W(dc, t.as_ptr(), t.len() as i32, &mut sz);
            sz.cx
        };

        let dim = |stale: bool, c: Rgba| if stale { Rgba(c.0, c.1, c.2, c.3 * 0.5) } else { c };
        let all_stale = rows.iter().all(|r| r.stale);
        let pad_l = s(8.0);
        let icon = if m.hide_icon { 0 } else { s(18.0) };
        let gap_icon = if m.hide_icon { 0 } else { s(7.0) };
        let gap_lv = if m.hide_labels { 0 } else { s(5.0) };
        let gap_div = s(10.0);
        let div_w = s(1.0).max(1);
        let pad_r = s(4.0);
        let div_h = if per_col == 1 { s(20.0) } else { (row_h as i32 - s(2.0)).max(1) };

        let mut x = pad_l;
        let icon_x = x;
        x += icon + gap_icon;
        let mut segs = Vec::new();
        let mut dividers = Vec::new();
        let enc = |t: &str| -> Vec<u16> { t.encode_utf16().collect() };
        let columns: Vec<&[Row]> = rows.chunks(rows_per_col).collect();
        for (ci, col) in columns.iter().enumerate() {
            // Line the column's rows up: each field is as wide as its widest value.
            let labels = [enc("5h"), enc("W")];
            let label_w = if m.hide_labels {
                [0, 0]
            } else {
                [measure(&labels[0], f_label), measure(&labels[1], f_label)]
            };
            let values: Vec<[Vec<u16>; 2]> = col
                .iter()
                .map(|r| [enc(&value_text(r.five, m.show_left)), enc(&value_text(r.week, m.show_left))])
                .collect();
            let mut value_w = [0; 2];
            for v in &values {
                for g in 0..2 {
                    value_w[g] = value_w[g].max(measure(&v[g], f_value));
                }
            }
            let mut group_x = [0; 2];
            let mut div_x = [0; 2];
            for g in 0..2 {
                group_x[g] = x;
                x += label_w[g] + gap_lv + value_w[g] + gap_div;
                div_x[g] = x;
                x += div_w;
                if g == 0 {
                    x += gap_div;
                }
            }
            for (ri, (row, vals)) in col.iter().zip(&values).enumerate() {
                let (y0, y1) = row_band(ri);
                for g in 0..2 {
                    let val = if g == 0 { row.five } else { row.week };
                    if !m.hide_labels {
                        segs.push(Seg { text: labels[g].clone(), bold: false, x: group_x[g], w: label_w[g], y0, y1, color: pal.label });
                    }
                    let vw = measure(&vals[g], f_value);
                    segs.push(Seg {
                        text: vals[g].clone(),
                        bold: true,
                        x: group_x[g] + label_w[g] + gap_lv,
                        w: vw,
                        y0,
                        y1,
                        color: dim(row.stale, value_color(val, &pal)),
                    });
                    let dy = y0 + ((y1 - y0) - div_h) / 2;
                    dividers.push(Divider { x: div_x[g], y0: dy.max(0), y1: (dy + div_h).min(h) });
                }
            }
            if ci + 1 < columns.len() {
                x += gap_div;
            }
        }
        let w = x + pad_r;

        // Text mask: white text on black.
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            },
            ..std::mem::zeroed()
        };
        let mut bits = std::ptr::null_mut();
        let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        let old_bmp = SelectObject(dc, bmp);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, 0x00FFFFFF);
        for seg in &segs {
            SelectObject(dc, if seg.bold { f_value } else { f_label });
            let mut rc = RECT { left: seg.x, top: seg.y0, right: seg.x + seg.w + s(2.0), bottom: seg.y1 };
            DrawTextW(dc, seg.text.as_ptr(), seg.text.len() as i32, &mut rc,
                DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX);
        }
        GdiFlush();
        let n = (w * h) as usize;
        let mask: Vec<u8> = std::slice::from_raw_parts(bits as *const [u8; 4], n)
            .iter()
            .map(|p| p[0].max(p[1]).max(p[2]))
            .collect();
        SelectObject(dc, old_bmp);
        DeleteObject(bmp);
        DeleteDC(dc);
        DeleteObject(f_label);
        DeleteObject(f_value);

        // Compose (premultiplied floats).
        let mut buf = vec![[0f32; 4]; n];
        let over = |buf: &mut [[f32; 4]], i: usize, c: Rgba, cov: f32| {
            let a = c.3 * cov;
            if a <= 0.0 {
                return;
            }
            let d = &mut buf[i];
            let k = 1.0 - a;
            d[0] = c.2 * a + d[0] * k;
            d[1] = c.1 * a + d[1] * k;
            d[2] = c.0 * a + d[2] * k;
            d[3] = a + d[3] * k;
        };

        // Hover pill (rounded rect, like taskbar buttons).
        let (top, bottom) = (s(4.0) as f32, (h - s(4.0)) as f32);
        if m.hover {
            let r = s(4.0) as f32;
            for y in 0..h {
                for xx in 0..w {
                    let cov = rounded_rect_cov(xx as f32 + 0.5, y as f32 + 0.5, 0.0, top, w as f32, bottom, r);
                    over(&mut buf, (y * w + xx) as usize, pal.hover, cov);
                }
            }
        }

        // Dividers.
        for d in &dividers {
            for y in d.y0..d.y1 {
                for xx in d.x..d.x + div_w {
                    over(&mut buf, (y * w + xx) as usize, pal.divider, 1.0);
                }
            }
        }

        // Claude mark.
        let iy = (h - icon) / 2;
        let mark = draw_mark(icon);
        for y in 0..icon {
            for xx in 0..icon {
                let cov = mark[(y * icon + xx) as usize];
                over(&mut buf, ((iy + y) * w + icon_x + xx) as usize, dim(all_stale, CLAUDE_ORANGE), cov);
            }
        }

        // Text.
        for seg in &segs {
            for y in seg.y0.max(0)..seg.y1.min(h) {
                for xx in seg.x.max(0)..(seg.x + seg.w + s(2.0)).min(w) {
                    let i = (y * w + xx) as usize;
                    let cov = mask[i] as f32 / 255.0;
                    if cov > 0.0 {
                        over(&mut buf, i, seg.color, cov);
                    }
                }
            }
        }

        // Keep the whole widget hit-testable (alpha 0 pixels are click-through).
        let px = buf
            .iter()
            .map(|p| {
                let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                if p[3] < 1.0 / 255.0 {
                    [0, 0, 0, 1]
                } else {
                    [q(p[0]), q(p[1]), q(p[2]), q(p[3])]
                }
            })
            .collect();
        Frame { w, h, px }
    }
}

fn rounded_rect_cov(x: f32, y: f32, l: f32, t: f32, r: f32, b: f32, rad: f32) -> f32 {
    let cx = x.clamp(l + rad, r - rad);
    let cy = y.clamp(t + rad, b - rad);
    let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - rad;
    (0.5 - d).clamp(0.0, 1.0)
}

/// Claude's asterisk-like mark: 12 rounded spokes of slightly varying length.
fn draw_mark(size: i32) -> Vec<f32> {
    const SS: i32 = 4; // supersampling
    let n = 12;
    let c = size as f32 / 2.0;
    let r = size as f32 / 2.0;
    let lens = [1.0, 0.82, 0.95, 0.78, 1.0, 0.86, 0.92, 0.8, 0.98, 0.84, 0.9, 0.8];
    let half_w = (size as f32 * 0.052).max(0.8);
    let spokes: Vec<(f32, f32)> = (0..n)
        .map(|i| {
            let a = (i as f32) * std::f32::consts::TAU / n as f32 + 0.13;
            let len = (r - half_w) * lens[i as usize];
            (a.cos() * len, a.sin() * len)
        })
        .collect();
    let mut out = vec![0f32; (size * size) as usize];
    for y in 0..size {
        for x in 0..size {
            let mut hits = 0;
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = x as f32 + (sx as f32 + 0.5) / SS as f32 - c;
                    let py = y as f32 + (sy as f32 + 0.5) / SS as f32 - c;
                    let inside = spokes.iter().any(|&(ex, ey)| {
                        // distance from point to segment (0,0)-(ex,ey)
                        let t = ((px * ex + py * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
                        let (dx, dy) = (px - ex * t, py - ey * t);
                        // taper: thinner toward the tip
                        let hw = half_w * (1.0 + 0.25 * t);
                        dx * dx + dy * dy <= hw * hw
                    });
                    if inside {
                        hits += 1;
                    }
                }
            }
            out[(y * size + x) as usize] = hits as f32 / (SS * SS) as f32;
        }
    }
    out
}
