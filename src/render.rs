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

const RING_BLUE: Rgba = Rgba(0.298, 0.608, 0.961, 1.0); // #4C9BF5
pub const CLAUDE_ORANGE: Rgba = Rgba(0.851, 0.467, 0.341, 1.0); // #D97757

/// One account's numbers.
pub struct Row {
    pub five: Option<f64>,
    pub week: Option<f64>,
    /// Fable's weekly limit; its group only shows when some account has one.
    pub fable: Option<f64>,
    /// A change being animated: the values before it (5h, week, Fable) and progress 0 to 1.
    pub roll: Option<([Option<f64>; 3], f32)>,
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
    /// Draw the pixel-art mark instead of the asterisk.
    pub pixel_icon: bool,
    /// Icon size in pixels at 100% scaling.
    pub icon_size: u32,
    /// Draw a progress ring for each limit.
    pub rings: bool,
    /// With rings: put the number inside a larger ring instead of beside a small one.
    pub ring_text: bool,
    /// With rings: only the ring turns orange / red near the limit, the number keeps its color.
    pub ring_color_only: bool,
    /// With numbers inside: smaller rings that keep space above and below (small taskbars).
    pub ring_space: bool,
    /// Leave out the `5h` / `W` labels, showing only the numbers.
    pub hide_labels: bool,
    /// Leave out the 5-hour, weekly or Fable group.
    pub hide_five: bool,
    pub hide_week: bool,
    pub hide_fable: bool,
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
        // Claude Code's thresholds: warning above 75 %, critical above 95 %.
        Some(p) if p > 95.0 => pal.crit,
        Some(p) if p > 75.0 => pal.warn,
        _ => pal.value,
    }
}

unsafe fn make_font(px: i32, faces: &[&str]) -> HFONT {
    for (i, face) in faces.iter().enumerate() {
        let f = CreateFontW(
            -px, 0, 0, 0, FW_NORMAL as i32, 0, 0, 0,
            DEFAULT_CHARSET as u32, OUT_TT_PRECIS as u32, CLIP_DEFAULT_PRECIS as u32,
            DEFAULT_QUALITY as u32, (DEFAULT_PITCH | FF_SWISS) as u32,
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
    /// The small font that fits inside a ring.
    small: bool,
    x: i32,
    w: i32,
    /// Vertical band of the account row this text belongs to.
    y0: i32,
    y1: i32,
    color: Rgba,
    /// Vertical motion blur radius in pixels.
    blur: i32,
    /// Room past `w` for glyph overhang.
    slack: i32,
}

struct Ring {
    x: i32,
    y: i32,
    /// Filled part, 0 to 1, clockwise from the top.
    frac: f32,
    color: Rgba,
}

struct Divider {
    x0: i32,
    x1: i32,
    y0: i32,
    y1: i32,
}

/// Renders the widget for a taskbar of height `h` at `scale` (DPI / 96), or with
/// `vert = Some(width)` for a vertical taskbar: one line per limit, stacked under
/// the icon, shrunk until it fits that width.
pub fn render(m: &Model, h: i32, scale: f32, vert: Option<i32>) -> Frame {
    let Some(avail) = vert else { return layout(m, h, scale, false) };
    let mut k = 1.0;
    loop {
        let f = layout(m, 0, scale * k, true);
        if f.w <= avail || k < 0.4 {
            return f;
        }
        k *= avail as f32 / f.w as f32 * 0.97;
    }
}

/// `vertical`: `h` is unused and the height follows the content.
fn layout(m: &Model, h: i32, scale: f32, vertical: bool) -> Frame {
    let pal = palette(m.dark);
    let s = |v: f32| (v * scale).round() as i32;
    // Vertical lines are sized as on a 32 px tall taskbar.
    let mut h = if vertical { s(32.0) } else { h };
    let empty = [Row { five: None, week: None, fable: None, roll: None, stale: false }];
    let rows: &[Row] = if m.rows.is_empty() { &empty } else { &m.rows };
    // Vertical: every account in one column, one line per limit.
    let rows_per_col = if vertical { rows.len() } else { m.rows_per_col.clamp(1, MAX_ROWS_PER_COLUMN) };
    let per_col = if vertical { 1 } else { rows.len().min(rows_per_col) };
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
        let inside = m.rings && m.ring_text;
        // Ring diameter: big enough for the number inside, or about text height beside it.
        // "Space around the rings": 14 % of the row free above and below instead of
        // reaching the taskbar's edges (vertical lines have their own spacing).
        let fill = if inside && m.ring_space && !vertical { (row_h * 0.72).round() as i32 } else { row_h as i32 - s(2.0) };
        let ring_d = if !m.rings { 0 } else { fill.clamp(1, s(if inside { 34.0 } else { 16.0 })) };
        let ring_gap = if m.rings && !inside { s(5.0) } else { 0 };
        let mut small_px = ((ring_d as f32 * 0.32).round() as i32).max(1);
        let mut f_small = make_font(
            small_px,
            &["Segoe UI Variable Text Semibold", "Segoe UI Semibold"],
        );
        let f_label = make_font(font_px, &["Segoe UI Variable Text", "Segoe UI"]);
        let f_value = make_font(font_px, &["Segoe UI Variable Text Semibold", "Segoe UI Semibold"]);

        let dc = CreateCompatibleDC(std::ptr::null_mut());
        let measure = |t: &[u16], f: HFONT| -> i32 {
            SelectObject(dc, f);
            let mut sz = SIZE { cx: 0, cy: 0 };
            GetTextExtentPoint32W(dc, t.as_ptr(), t.len() as i32, &mut sz);
            sz.cx
        };
        // Blank space before the first glyph and after the last one.
        let bearings = |t: &[u16], f: HFONT| -> (i32, i32) {
            SelectObject(dc, f);
            let abc = |c: u16| {
                let mut a = std::mem::zeroed::<ABC>();
                GetCharABCWidthsW(dc, c as u32, c as u32, &mut a);
                a
            };
            match (t.first(), t.last()) {
                (Some(&a), Some(&b)) => (abc(a).abcA, abc(b).abcC),
                _ => (0, 0),
            }
        };
        if inside {
            // The widest number, "100%", keeps clear of the ring's inner edge.
            let room = ring_d as f32 * 0.85 - 2.0 * (ring_d as f32 * 0.1).max(1.5);
            let widest = measure(&"100%".encode_utf16().collect::<Vec<u16>>(), f_small) as f32;
            if widest > room {
                small_px = ((small_px as f32 * room / widest).floor() as i32).max(1);
                DeleteObject(f_small);
                f_small = make_font(small_px, &["Segoe UI Variable Text Semibold", "Segoe UI Semibold"]);
            }
        }

        let dim = |stale: bool, c: Rgba| if stale { Rgba(c.0, c.1, c.2, c.3 * 0.5) } else { c };
        let all_stale = rows.iter().all(|r| r.stale);
        let pad_l = s(8.0);
        let icon = if m.hide_icon { 0 } else { s(m.icon_size as f32).min(h) };
        let gap_icon = if m.hide_icon { 0 } else { s(7.0) };
        let gap_lv = if m.hide_labels { 0 } else { s(5.0) };
        let gap_div = s(10.0);
        let div_w = s(1.0).max(1);
        let pad_r = s(4.0);
        let div_h = if per_col == 1 { s(20.0) } else { (row_h as i32 - s(2.0)).max(1) };

        let mut x = pad_l;
        let mut icon_x = x;
        let pad_v = s(6.0);
        // Vertical: the icon gets a line of its own, then each limit one line.
        let mut y = pad_v + if icon > 0 { icon + pad_v } else { 0 };
        let line_h = ((font_px as f32 * 1.6).round() as i32).max(ring_d + s(4.0));
        let mut hlines = Vec::new();
        if !vertical {
            x += icon + gap_icon;
        }
        let mut segs = Vec::new();
        let mut dividers = Vec::new();
        let mut rings = Vec::new();
        let enc = |t: &str| -> Vec<u16> { t.encode_utf16().collect() };
        let columns: Vec<&[Row]> = rows.chunks(rows_per_col).collect();
        for (ci, col) in columns.iter().enumerate() {
            // Line the column's rows up: each field is as wide as its widest value.
            let labels = [enc("5h"), enc("W"), enc("F")];
            // Which of 5h / week / Fable are shown, as indexes into `labels`.
            let ids: Vec<usize> = [!m.hide_five, !m.hide_week, !m.hide_fable && rows.iter().any(|r| r.fable.is_some())]
                .iter()
                .enumerate()
                .filter_map(|(k, on)| on.then_some(k))
                .collect();
            let groups = ids.len();
            let mut label_w: Vec<i32> = ids
                .iter()
                .map(|&k| &labels[k])
                .map(|l| if m.hide_labels { 0 } else { measure(l, f_label) })
                .collect();
            let values: Vec<Vec<Vec<u16>>> = col
                .iter()
                .map(|r| ids.iter().map(|&k| enc(&value_text([r.five, r.week, r.fable][k], m.show_left))).collect())
                .collect();
            let mut value_w = vec![0; groups];
            for v in &values {
                for g in 0..groups {
                    value_w[g] = value_w[g].max(measure(&v[g], if inside { f_small } else { f_value }));
                }
            }
            let mut group_x = vec![pad_l; groups];
            let mut div_x = vec![0; groups];
            if vertical {
                // Labels share the widest label's width so the values line up.
                let lw = label_w.iter().copied().max().unwrap_or(0);
                label_w.iter_mut().for_each(|l| *l = lw);
                let vw = value_w.iter().copied().max().unwrap_or(0);
                x = pad_l + lw + gap_lv + if inside { ring_d } else { ring_d + ring_gap + vw };
            } else {
                for g in 0..groups {
                    group_x[g] = x;
                    x += label_w[g] + gap_lv + if inside { ring_d } else { ring_d + ring_gap + value_w[g] } + gap_div;
                    div_x[g] = x;
                    x += div_w;
                    if g + 1 < groups {
                        x += gap_div;
                    }
                }
            }
            for (ri, (row, vals)) in col.iter().zip(&values).enumerate() {
                let (y0, y1) = row_band(ri);
                // Eased progress of a rolling change (ease in-out cubic) and its speed, for the blur.
                let (p, speed) = match row.roll {
                    Some((_, t)) if t < 0.5 => (4.0 * t * t * t, 12.0 * t * t),
                    Some((_, t)) => {
                        let u = 2.0 - 2.0 * t.min(1.0);
                        (1.0 - u * u * u / 2.0, 3.0 * u * u)
                    }
                    None => (1.0, 0.0),
                };
                for g in 0..groups {
                    let (y0, y1) = if vertical {
                        y += line_h;
                        (y - line_h, y)
                    } else {
                        (y0, y1)
                    };
                    let val = [row.five, row.week, row.fable][ids[g]];
                    if !m.hide_labels {
                        segs.push(Seg { text: labels[ids[g]].clone(), bold: false, small: false, x: group_x[g], w: label_w[g], y0, y1, color: pal.label, blur: 0, slack: s(2.0) });
                    }
                    let dy = y0 + ((y1 - y0) - div_h) / 2;
                    if !vertical {
                        dividers.push(Divider { x0: div_x[g], x1: div_x[g] + div_w, y0: dy.max(0), y1: (dy + div_h).min(h) });
                    }
                    let content_x = group_x[g] + label_w[g] + gap_lv;
                    let color = value_color(val, &pal);
                    if m.rings {
                        // The ring shows the same quantity as the number.
                        let frac = |v: Option<f64>| {
                            let used = v.map_or(0.0, |q| q.round().clamp(0.0, 100.0) as f32 / 100.0);
                            if v.is_some() && m.show_left { 1.0 - used } else { used }
                        };
                        let to = frac(val);
                        rings.push(Ring {
                            x: content_x,
                            y: y0 + ((y1 - y0) - ring_d) / 2,
                            frac: row.roll.map_or(to, |(from, _)| frac(from[ids[g]]) + (to - frac(from[ids[g]])) * p),
                            color: dim(row.stale, if color == pal.value { RING_BLUE } else { color }),
                        });
                    }
                    let vf = if inside { f_small } else { f_value };
                    let text_x = |t: &[u16]| {
                        if inside {
                            let (a, c) = bearings(t, vf);
                            content_x + (ring_d - (measure(t, vf) - a - c)) / 2 - a
                        } else {
                            content_x + ring_d + ring_gap
                        }
                    };
                    let color = dim(row.stale, if m.rings && m.ring_color_only { pal.value } else { color });
                    let old = row
                        .roll
                        .map(|(from, _)| enc(&value_text(from[ids[g]], m.show_left)))
                        .filter(|o| *o != vals[g]);
                    let Some(old) = old else {
                        let vw = measure(&vals[g], vf);
                        segs.push(Seg { text: vals[g].clone(), bold: true, small: inside, x: text_x(&vals[g]), w: vw, y0, y1, color, blur: 0, slack: s(2.0) });
                        continue;
                    };
                    // Rolling: each changed character's old glyph leaves upward while the new
                    // one arrives from below, one line apart, each drawn in its own line box.
                    let mut line = ((if inside { small_px } else { font_px }) as f32 * 1.25).round() as i32;
                    line += (line - (y1 - y0)) & 1; // same parity as the row, so still glyphs don't shift
                    let top = y0 + ((y1 - y0) - line) / 2;
                    let moved = (p * line as f32).round() as i32;
                    let blur = (speed * line as f32 * 0.048).round() as i32;
                    for (text, other, is_new) in [(&old, &vals[g], false), (&vals[g], &old, true)] {
                        let base = text_x(text);
                        for i in 0..text.len() {
                            let changed = other.get(i) != Some(&text[i]);
                            if !changed && !is_new {
                                continue; // unchanged: drawn once, from the new text
                            }
                            let (dy, alpha) = match (changed, is_new) {
                                (false, _) => (0, 1.0),
                                (true, true) => (line - moved, p),
                                (true, false) => (-moved, 1.0 - p),
                            };
                            let cx = measure(&text[..i], vf);
                            segs.push(Seg {
                                text: vec![text[i]],
                                bold: true,
                                small: inside,
                                x: base + cx,
                                w: measure(&text[..=i], vf) - cx,
                                y0: top + dy,
                                y1: top + dy + line,
                                color: Rgba(color.0, color.1, color.2, color.3 * alpha),
                                blur: if changed { blur } else { 0 },
                                slack: 0,
                            });
                        }
                    }
                }
                if vertical && ri + 1 < col.len() {
                    // A rule between accounts.
                    hlines.push(y + gap_div / 2);
                    y += gap_div;
                }
            }
            if ci + 1 < columns.len() {
                x += gap_div;
            }
        }
        let w = if vertical { x + pad_l } else { x + pad_r };
        if vertical {
            h = y + pad_v;
            icon_x = (w - icon) / 2;
            for &hy in &hlines {
                dividers.push(Divider { x0: pad_l, x1: w - pad_l, y0: hy, y1: hy + div_w });
            }
        }

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
            SelectObject(dc, if seg.small { f_small } else if seg.bold { f_value } else { f_label });
            let mut rc = RECT { left: seg.x, top: seg.y0, right: seg.x + seg.w + seg.slack, bottom: seg.y1 };
            DrawTextW(dc, seg.text.as_ptr(), seg.text.len() as i32, &mut rc,
                DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX);
        }
        GdiFlush();
        let n = (w * h) as usize;
        // ClearType coverage per subpixel, as R, G, B (the DIB is BGRA). On a light
        // taskbar it is averaged to gray: one alpha can't carry per-channel coverage,
        // and the error that leaves (bg x coverage difference) shows on light
        // backgrounds only.
        let mask: Vec<[u8; 3]> = std::slice::from_raw_parts(bits as *const [u8; 4], n)
            .iter()
            .map(|p| {
                if m.dark {
                    [p[2], p[1], p[0]]
                } else {
                    let g = ((p[0] as u32 + p[1] as u32 + p[2] as u32) / 3) as u8;
                    [g, g, g]
                }
            })
            .collect();
        SelectObject(dc, old_bmp);
        DeleteObject(bmp);
        DeleteDC(dc);
        DeleteObject(f_label);
        DeleteObject(f_value);
        DeleteObject(f_small);

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
                for xx in d.x0..d.x1 {
                    over(&mut buf, (y * w + xx) as usize, pal.divider, 1.0);
                }
            }
        }

        // Usage rings: a faint track, then the filled arc clockwise from the top.
        let rad = ring_d as f32 / 2.0;
        let stroke = (ring_d as f32 * 0.1).max(1.5);
        for r in &rings {
            for y in 0..ring_d {
                for xx in 0..ring_d {
                    let (dx, dy) = (xx as f32 + 0.5 - rad, y as f32 + 0.5 - rad);
                    let d = (dx * dx + dy * dy).sqrt();
                    let band = (stroke / 2.0 + 0.5 - (d - (rad - stroke / 2.0)).abs()).clamp(0.0, 1.0);
                    let i = (r.y + y) * w + r.x + xx;
                    if band <= 0.0 || r.y + y < 0 || r.y + y >= h || i as usize >= n {
                        continue;
                    }
                    over(&mut buf, i as usize, pal.divider, band);
                    let a = dx.atan2(-dy).rem_euclid(std::f32::consts::TAU);
                    let arc = if r.frac >= 1.0 {
                        1.0
                    } else if r.frac <= 0.0 {
                        0.0
                    } else {
                        // Distance along the ring to the arc's end, in pixels.
                        ((r.frac * std::f32::consts::TAU - a) * d + 0.5).clamp(0.0, 1.0)
                    };
                    over(&mut buf, i as usize, r.color, band * arc);
                }
            }
        }

        // Claude mark.
        let iy = if vertical { pad_v } else { (h - icon) / 2 };
        let mark = if m.pixel_icon { draw_pixel_mark(icon) } else { draw_mark(icon) };
        for y in 0..icon {
            for xx in 0..icon {
                let cov = mark[(y * icon + xx) as usize];
                over(&mut buf, ((iy + y) * w + icon_x + xx) as usize, dim(all_stale, CLAUDE_ORANGE), cov);
            }
        }

        // Text.
        for seg in &segs {
            for y in seg.y0.max(0)..seg.y1.min(h) {
                for xx in seg.x.max(0)..(seg.x + seg.w + seg.slack).min(w) {
                    let i = (y * w + xx) as usize;
                    // Motion blur: a vertical box average kept inside the glyph's own line box.
                    let mut sum = [0u32; 3];
                    for yy in (y - seg.blur..=y + seg.blur).filter(|yy| (seg.y0.max(0)..seg.y1.min(h)).contains(yy)) {
                        let mk = mask[(yy * w + xx) as usize];
                        (0..3).for_each(|c| sum[c] += mk[c] as u32);
                    }
                    let cov = sum.map(|v| v as f32 / (255.0 * (2 * seg.blur + 1) as f32));
                    if cov.iter().any(|&c| c > 0.0) {
                        // Per-channel "over" in premultiplied BGRA; alpha takes the
                        // strongest channel.
                        let c = seg.color;
                        let a = cov.map(|v| c.3 * v);
                        let d = &mut buf[i];
                        d[0] = c.2 * a[2] + d[0] * (1.0 - a[2]);
                        d[1] = c.1 * a[1] + d[1] * (1.0 - a[1]);
                        d[2] = c.0 * a[0] + d[2] * (1.0 - a[0]);
                        let am = a[0].max(a[1]).max(a[2]);
                        d[3] = am + d[3] * (1.0 - am);
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

/// Pixel-art mark (Claude Code's little creature), as wide as the asterisk.
/// Cells may cover part of a pixel; edge pixels get the covered fraction.
fn draw_pixel_mark(size: i32) -> Vec<f32> {
    const ART: [&[u8; 12]; 8] = [
        b"..########..",
        b"..########..",
        b"..#.####.#..",
        b"############",
        b"..########..",
        b"..########..",
        b"..#.#..#.#..",
        b"..#.#..#.#..",
    ];
    let cell = size as f32 / 12.0;
    let oy = (size as f32 - 8.0 * cell) / 2.0;
    // How much of pixel `p` the span [a, b) covers.
    let overlap = |p: i32, a: f32, b: f32| (b.min(p as f32 + 1.0) - a.max(p as f32)).max(0.0);
    let mut out = vec![0f32; (size * size) as usize];
    for (r, row) in ART.iter().enumerate() {
        for (c, _) in row.iter().enumerate().filter(|(_, ch)| **ch == b'#') {
            let (x0, y0) = (c as f32 * cell, oy + r as f32 * cell);
            let (x1, y1) = (x0 + cell, y0 + cell);
            for y in (y0.floor() as i32).max(0)..(y1.ceil() as i32).min(size) {
                for x in (x0.floor() as i32).max(0)..(x1.ceil() as i32).min(size) {
                    let v = &mut out[(y * size + x) as usize];
                    *v = (*v + overlap(x, x0, x1) * overlap(y, y0, y1)).min(1.0);
                }
            }
        }
    }
    out
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
