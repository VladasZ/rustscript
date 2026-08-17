//! Render one PNG per benchmark case, light themed, fixed color per
//! language, linear scale. Each compute case gets three panels, wall-clock,
//! self timed compute, and peak memory. The startup cases skip the compute
//! panel.
//!
//! Usage: cargo run --release --bin chart

use num_traits::AsPrimitive;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use plotters::coord::Shift;
use plotters::prelude::*;
use plotters::style::text_anchor::{HPos, Pos, VPos};
use rustscript_bench::{CaseResult, Report};

/// Pixel density. Layout is in logical units and everything is drawn at
/// double resolution so the PNGs stay sharp on hidpi screens.
const S: i32 = 2;

fn s(v: i32) -> i32 {
    v * S
}

const BG: RGBColor = RGBColor(252, 252, 250);
const INK: RGBColor = RGBColor(40, 40, 44);
const MUTED: RGBColor = RGBColor(130, 130, 138);
const GRID: RGBColor = RGBColor(224, 224, 228);

const LANG_ORDER: [&str; 4] = ["native", "rustscript", "node", "python"];

/// Bar geometry in logical units. Bars are packed 5 units apart and the
/// panel width follows from the packed group, so there is no dead space.
const BAR_W: i32 = 52;
const BAR_GAP: i32 = 5;
const PANEL_MARGIN: i32 = 16;

fn panel_width(bars: i32) -> i32 {
    bars * BAR_W + (bars - 1) * BAR_GAP + 2 * PANEL_MARGIN
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

/// The chart title: the case name spelled out so the task is obvious.
/// File names keep the short case name from `results.json`.
fn display_title(name: &str) -> &'static str {
    match name {
        "hello" => "hello world",
        "big_script" => "big script startup",
        "multifile_startup" => "multi-file startup",
        "fib" => "recursive fibonacci",
        "sieve" => "sieve of eratosthenes",
        "mandelbrot" => "mandelbrot",
        "collatz" => "collatz",
        "binary_trees" => "binary trees",
        "string_builder" => "string building",
        "higher_order" => "map filter fold",
        "sort" => "comparator sort",
        "sort_key" => "sort by key",
        "hashmap_int" => "int hashmap",
        "nbody" => "n-body",
        "json_serialize" => "json serialize",
        "stdout_lines" => "stdout lines",
        "word_count" => "word count",
        "json" => "json parse",
        "regex" => "regex",
        "file_transform" => "file transform",
        "process_spawn" => "process spawn",
        "async_tasks" => "async tasks",
        "http_local" => "local http",
        _ => "automation script",
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
        let out = dir.join(format!("{}.png", c.name));
        render_case(&out, c)?;
        println!("wrote {}", out.display());
    }
    Ok(())
}

/// One PNG for one case.
/// The panels a case renders: wall-clock, compute-only when self timed, and
/// peak memory when measured. The time panels share one axis so bar heights
/// compare directly.
fn case_panels(c: &CaseResult) -> Vec<Panel> {
    let mut panels: Vec<Panel> = Vec::new();

    let wall: Vec<_> = LANG_ORDER
        .iter()
        .filter_map(|l| {
            c.wall_of(l)
                .map(|w| ((*l).to_string(), w.median, color_for(l)))
        })
        .collect();
    let comp: Vec<_> = LANG_ORDER
        .iter()
        .filter_map(|l| {
            c.compute_of(l)
                .map(|w| ((*l).to_string(), w.median, color_for(l)))
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
                    (*l).to_string(),
                    AsPrimitive::<f64>::as_(m.median_bytes),
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
    panels
}

fn render_case(out: &Path, c: &CaseResult) -> Result<()> {
    let panels = case_panels(c);
    let bars = i32::try_from(panels.first().map_or(4, |p| p.bars.len())).expect("bars fit i32");
    let w = i32::try_from(panels.len()).expect("panel count fits i32") * panel_width(bars);
    let h = 500i32;
    let dims = (
        u32::try_from(s(w)).expect("chart width fits u32"),
        u32::try_from(s(h)).expect("chart height fits u32"),
    );
    let area = BitMapBackend::new(out, dims).into_drawing_area();
    area.fill(&BG)?;

    let (head, body) = area.split_vertically(s(60));
    head.draw(&Text::new(
        display_title(&c.name),
        (s(28), s(18)),
        ("sans-serif", s(30)).into_font().color(&INK),
    ))?;

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
    let w = i32::try_from(w).expect("chart width fits i32") / S;
    let h = i32::try_from(h).expect("chart height fits i32") / S;
    let (top, bottom) = (46i32, 34i32);
    let plot_l = PANEL_MARGIN;
    let plot_r = w - PANEL_MARGIN;
    let plot_t = top;
    let plot_b = h - bottom;
    let plot_h = plot_b - plot_t;

    area.draw(&Text::new(
        p.title.clone(),
        (s(plot_l), s(16)),
        ("sans-serif", s(16)).into_font().color(&INK),
    ))
    .map_err(de)?;
    area.draw(&PathElement::new(
        vec![(s(plot_l), s(plot_b)), (s(plot_r), s(plot_b))],
        GRID.stroke_width(u32::try_from(S).expect("scale fits u32")),
    ))
    .map_err(de)?;

    for (i, (label, value, color)) in p.bars.iter().enumerate() {
        let x0 = plot_l + (BAR_W + BAR_GAP) * i32::try_from(i).expect("bar count fits i32");
        let x1 = x0 + BAR_W;
        let bh = AsPrimitive::<i32>::as_(((value / p.axis_hi) * f64::from(plot_h)).round());
        let y0 = plot_b - bh.max(1);
        area.draw(&Rectangle::new(
            [(s(x0), s(y0)), (s(x1), s(plot_b))],
            color.filled(),
        ))
        .map_err(de)?;
        area.draw(&Text::new(
            (p.fmt)(*value),
            (s(x0 + BAR_W / 2), s(y0 - 16)),
            ("sans-serif", s(13))
                .into_font()
                .color(&INK)
                .pos(Pos::new(HPos::Center, VPos::Top)),
        ))
        .map_err(de)?;
        area.draw(&Text::new(
            label.clone(),
            (s(x0 + BAR_W / 2), s(plot_b + 8)),
            ("sans-serif", s(12))
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
