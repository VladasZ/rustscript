//! Render one PNG per benchmark case and tier, light themed, fixed color per
//! language, linear scale. Each compute case gets three panels, wall-clock,
//! self timed compute, and peak memory. The startup cases skip the compute
//! panel. The big tier renders to `<case>_big.png`.
//!
//! Usage: cargo run --release --bin chart

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use plotters::coord::Shift;
use plotters::prelude::*;
use plotters::style::text_anchor::{HPos, Pos, VPos};
use rustscript_bench::{CaseResult, Meta, Report};

const BG: RGBColor = RGBColor(252, 252, 250);
const INK: RGBColor = RGBColor(40, 40, 44);
const MUTED: RGBColor = RGBColor(130, 130, 138);
const GRID: RGBColor = RGBColor(224, 224, 228);

const LANG_ORDER: [&str; 4] = ["native", "rustscript", "node", "python"];

fn display_name(lang: &str) -> &str {
    match lang {
        "native" => "native rust",
        other => other,
    }
}

fn color_for(lang: &str) -> RGBColor {
    match lang {
        "native" => RGBColor(64, 110, 180),
        "rustscript" => RGBColor(224, 116, 38),
        "node" => RGBColor(38, 166, 154),
        "python" => RGBColor(56, 150, 96),
        _ => MUTED,
    }
}

/// One bar panel, values in the unit `fmt` renders.
struct Panel {
    title: String,
    bars: Vec<(String, f64, RGBColor)>,
    axis_hi: f64,
    fmt: fn(f64) -> String,
}

fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("no parent")?;
    let results = root.join("bench/results/results.json");
    let report: Report =
        serde_json::from_str(&fs::read_to_string(&results).with_context(|| {
            format!(
                "read {}, run `cargo run --release --bin bench` first",
                results.display()
            )
        })?)?;

    let dir = root.join("bench/results");
    for c in &report.cases {
        let file = if c.tier == "big" {
            format!("{}_big.png", c.name)
        } else {
            format!("{}.png", c.name)
        };
        let out = dir.join(file);
        render_case(&out, c, &report.meta)?;
        println!("wrote {}", out.display());
    }
    Ok(())
}

/// One PNG for one case at one tier.
fn render_case(out: &Path, c: &CaseResult, meta: &Meta) -> Result<()> {
    let mut panels: Vec<Panel> = Vec::new();

    let wall: Vec<_> = LANG_ORDER
        .iter()
        .filter_map(|l| {
            c.wall_of(l)
                .map(|w| (display_name(l).to_string(), w.median, color_for(l)))
        })
        .collect();
    let comp: Vec<_> = LANG_ORDER
        .iter()
        .filter_map(|l| {
            c.compute_of(l)
                .map(|w| (display_name(l).to_string(), w.median, color_for(l)))
        })
        .collect();
    // The time panels share one axis so bar heights compare directly.
    let tmax = wall
        .iter()
        .chain(comp.iter())
        .map(|b| b.1)
        .fold(0f64, f64::max);
    let taxis = if tmax > 0.0 { tmax * 1.18 } else { 1.0 };
    panels.push(Panel {
        title: "wall-clock   startup plus run".to_string(),
        bars: wall,
        axis_hi: taxis,
        fmt: fmt_time,
    });
    if !comp.is_empty() {
        panels.push(Panel {
            title: "compute-only   self timed".to_string(),
            bars: comp,
            axis_hi: taxis,
            fmt: fmt_time,
        });
    }

    let mem: Vec<_> = LANG_ORDER
        .iter()
        .filter_map(|l| {
            c.memory_of(l).map(|m| {
                (
                    display_name(l).to_string(),
                    m.median_bytes as f64,
                    color_for(l),
                )
            })
        })
        .collect();
    if !mem.is_empty() {
        let mmax = mem.iter().map(|b| b.1).fold(0f64, f64::max);
        panels.push(Panel {
            title: "peak memory   max rss".to_string(),
            bars: mem,
            axis_hi: if mmax > 0.0 { mmax * 1.18 } else { 1.0 },
            fmt: fmt_bytes,
        });
    }

    let w = match panels.len() {
        1 => 620u32,
        2 => 1080u32,
        _ => 1500u32,
    };
    let h = 560u32;
    let area = BitMapBackend::new(out, (w, h)).into_drawing_area();
    area.fill(&BG)?;

    let title = if c.tier == "big" {
        format!("{}   10x size", c.name)
    } else {
        c.name.clone()
    };
    let (head, body) = area.split_vertically(96);
    head.draw(&Text::new(
        title,
        (28, 22),
        ("sans-serif", 30).into_font().color(&INK),
    ))?;
    let commit = meta.git_commit.get(..8).unwrap_or(&meta.git_commit);
    let state = if meta.git_dirty {
        format!("{commit} DIRTY TREE")
    } else {
        commit.to_string()
    };
    head.draw(&Text::new(
        format!("same task, byte-identical output. medians. lower is better. {state}"),
        (30, 62),
        ("sans-serif", 15).into_font().color(&MUTED),
    ))?;
    // Legend, top right.
    let mut lx = (w as i32) - 470;
    for lang in LANG_ORDER {
        head.draw(&Rectangle::new(
            [(lx, 20), (lx + 18, 36)],
            color_for(lang).filled(),
        ))?;
        head.draw(&Text::new(
            display_name(lang),
            (lx + 24, 22),
            ("sans-serif", 14).into_font().color(&INK),
        ))?;
        lx += 118;
    }

    let cols = body.split_evenly((1, panels.len()));
    for (cell, p) in cols.iter().zip(panels.iter()) {
        panel(cell, p)?;
    }
    area.present()?;
    Ok(())
}

/// Draw one bar panel on a linear scale.
fn panel<DB>(area: &DrawingArea<DB, Shift>, p: &Panel) -> Result<()>
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

    area.draw(&Text::new(
        p.title.clone(),
        (left, 16),
        ("sans-serif", 16).into_font().color(&INK),
    ))
    .map_err(de)?;
    area.draw(&PathElement::new(
        vec![(plot_l, plot_b), (plot_r, plot_b)],
        GRID.stroke_width(1),
    ))
    .map_err(de)?;

    if p.bars.is_empty() {
        return Ok(());
    }
    let n = p.bars.len() as i32;
    let slot = plot_w / n;
    let bw = (slot as f64 * 0.5) as i32;
    for (i, (label, value, color)) in p.bars.iter().enumerate() {
        let cx = plot_l + slot * i as i32 + slot / 2;
        let bh = ((value / p.axis_hi) * plot_h as f64).round() as i32;
        let x0 = cx - bw / 2;
        let x1 = cx + bw / 2;
        let y0 = plot_b - bh.max(1);
        area.draw(&Rectangle::new([(x0, y0), (x1, plot_b)], color.filled()))
            .map_err(de)?;
        area.draw(&Text::new(
            (p.fmt)(*value),
            (cx, y0 - 16),
            ("sans-serif", 14)
                .into_font()
                .color(&INK)
                .pos(Pos::new(HPos::Center, VPos::Top)),
        ))
        .map_err(de)?;
        area.draw(&Text::new(
            label.clone(),
            (cx, plot_b + 8),
            ("sans-serif", 14)
                .into_font()
                .color(&MUTED)
                .pos(Pos::new(HPos::Center, VPos::Top)),
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

fn fmt_bytes(b: f64) -> String {
    let mb = b / 1e6;
    if mb >= 1000.0 {
        format!("{:.2}GB", mb / 1000.0)
    } else if mb >= 10.0 {
        format!("{mb:.0}MB")
    } else {
        format!("{mb:.1}MB")
    }
}
