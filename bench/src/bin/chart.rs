//! Render one PNG per benchmark case, light themed, fixed color per language,
//! linear scale. Each compute case gets two panels side by side, wall-clock and
//! self timed compute. The startup case has only the wall-clock panel.
//!
//! Usage: cargo run --release --bin chart

use std::path::Path;

use anyhow::{Context, Result};
use plotters::coord::Shift;
use plotters::prelude::*;
use plotters::style::text_anchor::{HPos, Pos, VPos};
use rustscript_bench::{CaseResult, Report};

const BG: RGBColor = RGBColor(252, 252, 250);
const INK: RGBColor = RGBColor(40, 40, 44);
const MUTED: RGBColor = RGBColor(130, 130, 138);
const GRID: RGBColor = RGBColor(224, 224, 228);

const LANG_ORDER: [&str; 4] = ["native", "rustscript", "bun", "python"];

fn color_for(lang: &str) -> RGBColor {
    match lang {
        "native" => RGBColor(64, 110, 180),
        "rustscript" => RGBColor(224, 116, 38),
        "bun" => RGBColor(196, 88, 152),
        "python" => RGBColor(56, 150, 96),
        _ => MUTED,
    }
}

fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().context("no parent")?;
    let results = root.join("bench/results/results.json");
    let report: Report =
        serde_json::from_str(&std::fs::read_to_string(&results).with_context(|| {
            format!("read {}, run `cargo run --release --bin bench` first", results.display())
        })?)?;

    let dir = root.join("bench/results");
    for c in &report.cases {
        let out = dir.join(format!("{}.png", c.name));
        render_case(&out, c)?;
        println!("wrote {}", out.display());
    }
    Ok(())
}

/// One PNG for one case.
fn render_case(out: &Path, c: &CaseResult) -> Result<()> {
    // Panels for this case.
    let mut panels: Vec<(String, Vec<(String, f64, RGBColor)>)> = Vec::new();
    let wall: Vec<_> = LANG_ORDER
        .iter()
        .filter_map(|l| c.wall_of(l).map(|w| (l.to_string(), w.median, color_for(l))))
        .collect();
    panels.push(("wall-clock   startup plus run".to_string(), wall));
    if c.kind != "startup" {
        let comp: Vec<_> = LANG_ORDER
            .iter()
            .filter_map(|l| c.compute_of(l).map(|w| (l.to_string(), w.min, color_for(l))))
            .collect();
        panels.push(("compute-only   self timed".to_string(), comp));
    }

    let w = if panels.len() == 1 { 620u32 } else { 1080u32 };
    let h = 560u32;
    let area = BitMapBackend::new(out, (w, h)).into_drawing_area();
    area.fill(&BG)?;

    let (head, body) = area.split_vertically(96);
    head.draw(&Text::new(c.name.clone(), (28, 22), ("sans-serif", 30).into_font().color(&INK)))?;
    head.draw(&Text::new(
        "same algorithm, byte identical output. lower is faster.",
        (30, 62),
        ("sans-serif", 15).into_font().color(&MUTED),
    ))?;
    // Legend, top right.
    let mut lx = (w as i32) - 470;
    for lang in LANG_ORDER {
        head.draw(&Rectangle::new([(lx, 20), (lx + 18, 36)], color_for(lang).filled()))?;
        head.draw(&Text::new(lang, (lx + 24, 22), ("sans-serif", 14).into_font().color(&INK)))?;
        lx += 118;
    }

    let cols = body.split_evenly((1, panels.len()));
    for (cell, (title, bars)) in cols.iter().zip(panels.iter()) {
        panel(cell, title, bars)?;
    }
    area.present()?;
    Ok(())
}

/// Draw one bar panel on a linear scale. Values are seconds.
fn panel<DB>(area: &DrawingArea<DB, Shift>, title: &str, bars: &[(String, f64, RGBColor)]) -> Result<()>
where
    DB: DrawingBackend,
    <DB as DrawingBackend>::ErrorType: 'static,
{
    let de = |e: DrawingAreaErrorKind<<DB as DrawingBackend>::ErrorType>| anyhow::anyhow!("{e:?}");
    let (w, h) = area.dim_in_pixel();
    let w = w as i32;
    let h = h as i32;
    let (left, right, top, bottom) = (24i32, 24i32, 46i32, 34i32);
    let plot_l = left;
    let plot_r = w - right;
    let plot_t = top;
    let plot_b = h - bottom;
    let plot_w = plot_r - plot_l;
    let plot_h = plot_b - plot_t;

    area.draw(&Text::new(title.to_string(), (left, 16), ("sans-serif", 16).into_font().color(&INK)))
        .map_err(de)?;
    area.draw(&PathElement::new(vec![(plot_l, plot_b), (plot_r, plot_b)], GRID.stroke_width(1)))
        .map_err(de)?;

    if bars.is_empty() {
        return Ok(());
    }
    let vmax = bars.iter().map(|b| b.1).fold(0f64, f64::max);
    let axis_hi = if vmax > 0.0 { vmax * 1.18 } else { 1.0 };

    let n = bars.len() as i32;
    let slot = plot_w / n;
    let bw = (slot as f64 * 0.5) as i32;
    for (i, (label, value, color)) in bars.iter().enumerate() {
        let cx = plot_l + slot * i as i32 + slot / 2;
        let bh = ((value / axis_hi) * plot_h as f64).round() as i32;
        let x0 = cx - bw / 2;
        let x1 = cx + bw / 2;
        let y0 = plot_b - bh.max(1);
        area.draw(&Rectangle::new([(x0, y0), (x1, plot_b)], color.filled())).map_err(de)?;
        area.draw(&Text::new(
            fmt_time(*value),
            (cx, y0 - 16),
            ("sans-serif", 14).into_font().color(&INK).pos(Pos::new(HPos::Center, VPos::Top)),
        ))
        .map_err(de)?;
        area.draw(&Text::new(
            label.clone(),
            (cx, plot_b + 8),
            ("sans-serif", 14).into_font().color(&MUTED).pos(Pos::new(HPos::Center, VPos::Top)),
        ))
        .map_err(de)?;
    }
    Ok(())
}

fn fmt_time(s: f64) -> String {
    if s >= 1.0 {
        format!("{s:.2}s")
    } else {
        let ms = s * 1e3;
        if ms >= 10.0 {
            format!("{ms:.0}ms")
        } else if ms >= 1.0 {
            format!("{ms:.1}ms")
        } else {
            format!("{ms:.2}ms")
        }
    }
}
