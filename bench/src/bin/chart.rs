//! Render one PNG per benchmark case, dark themed, fixed color per
//! language, linear scale. Each compute case gets three panels, total time,
//! self timed compute, and peak memory. The startup cases skip the compute
//! panel. The run also writes `bench/RESULTS.md`, the document that collects
//! every chart.
//!
//! Usage: cargo run --release --bin chart

use num_traits::AsPrimitive;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use plotters::coord::Shift;
use plotters::prelude::*;
use plotters::style::register_font;
use plotters::style::text_anchor::{HPos, Pos, VPos};
use rustscript_bench::{CaseResult, Meta, Report};

/// Pixel density. Layout is in logical units and everything is drawn at
/// double resolution so the PNGs stay sharp on hidpi screens.
const S: i32 = 2;

fn s(v: i32) -> i32 {
    v * S
}

const BG: RGBColor = RGBColor(24, 25, 28);
const INK: RGBColor = RGBColor(230, 230, 235);
const MUTED: RGBColor = RGBColor(148, 150, 158);
const GRID: RGBColor = RGBColor(62, 64, 70);

const LANG_ORDER: [&str; 4] = ["native", "rustscript", "node", "python"];

/// The only font the charts use. It is embedded because the pure Rust text
/// renderer has no system font source, so nothing is found by name.
const FONT: &[u8] = include_bytes!("../../fonts/Roboto-Regular.ttf");

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

/// The name under a bar. The `native` key in `results.json` is the compiled
/// Rust binary, so the bar names the language instead.
fn bar_label(lang: &str) -> String {
    match lang {
        "native" => "rust".to_string(),
        _ => lang.to_string(),
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
    register_font("sans-serif", FontStyle::Normal, FONT)
        .map_err(|_| anyhow!("the embedded Roboto file is not a valid font"))?;

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
        render_case(&out, c, &report.meta)?;
        println!("wrote {}", out.display());
    }

    let doc = root.join("bench/RESULTS.md");
    fs::write(&doc, results_markdown(&report)?)?;
    println!("wrote {}", doc.display());
    Ok(())
}

/// The document that collects every chart. It is written next to the charts
/// on every run, so it can never list a case the suite no longer measures.
fn results_markdown(report: &Report) -> Result<String> {
    let meta = &report.meta;
    let mut out = String::new();
    writeln!(out, "# Benchmark results\n")?;
    writeln!(
        out,
        r"Every case in the suite, one chart each, in run order. Each bar is the
median of that case's samples. [README.md](README.md) explains the method and
what every case measures.
"
    )?;
    writeln!(
        out,
        r"This file is written by `cargo run --release --bin chart` together with the
charts themselves. Edit that tool, not this file.
"
    )?;
    writeln!(out, "{}\n", machine_lines(meta))?;

    for c in &report.cases {
        let title = display_title(&c.name);
        writeln!(out, "## {title}\n")?;
        writeln!(out, "{}\n", case_line(c))?;
        writeln!(out, "![{title}](results/{}.png)\n", c.name)?;
        writeln!(
            out,
            "Scripts: [cases/{name}](cases/{name})\n",
            name = c.name
        )?;
    }
    Ok(out)
}

/// The machine, the runtimes, and the sample counts the recorded run used.
fn machine_lines(meta: &Meta) -> String {
    let cores = match meta.cpu_cores {
        0 => String::new(),
        n => format!(", {n} cores"),
    };
    let rustc = meta
        .rustc
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    let commit: String = meta.git_commit.chars().take(7).collect();
    let dirty = if meta.git_dirty { ", dirty tree" } else { "" };
    let settings = &meta.settings;
    format!(
        r"- machine: {cpu}{cores}, {os} {arch}
- runtimes: node {node}, {python}, {rustc}
- run: commit `{commit}`{dirty}, {warmups} warmup, {total} total samples, {compute} compute samples",
        cpu = meta.cpu,
        os = meta.os,
        arch = meta.arch,
        node = meta.node,
        python = meta.python,
        warmups = settings.warmups,
        total = settings.total_samples,
        compute = settings.compute_samples,
    )
}

/// The case name, its kind, and the arguments or fixture it ran with.
fn case_line(c: &CaseResult) -> String {
    let mut parts = vec![format!("`{}`", c.name), c.kind.clone()];
    parts.extend(c.parameters.iter().map(|p| format!("`{p}`")));
    parts.join(", ")
}

/// One PNG for one case.
/// The panels a case renders: total time, compute-only when self timed, and
/// peak memory when measured. The time panels share one axis so bar heights
/// compare directly.
fn case_panels(c: &CaseResult) -> Vec<Panel> {
    let mut panels: Vec<Panel> = Vec::new();

    let total: Vec<_> = LANG_ORDER
        .iter()
        .filter_map(|l| {
            c.total_of(l)
                .map(|w| (bar_label(l), w.median, color_for(l)))
        })
        .collect();
    let comp: Vec<_> = LANG_ORDER
        .iter()
        .filter_map(|l| {
            c.compute_of(l)
                .map(|w| (bar_label(l), w.median, color_for(l)))
        })
        .collect();
    // The time panels share one axis so bar heights compare directly.
    let tmax = total
        .iter()
        .chain(comp.iter())
        .map(|b| b.1)
        .fold(0f64, f64::max);
    let taxis = if tmax > 0.0 { tmax * 1.18 } else { 1.0 };
    panels.push(Panel {
        title: "total time   startup plus run".to_string(),
        bars: total,
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
                    bar_label(l),
                    AsPrimitive::<f64>::as_(m.median_bytes),
                    color_for(l),
                )
            })
        })
        .collect();
    if !mem.is_empty() {
        let mmax = mem.iter().map(|b| b.1).fold(0f64, f64::max);
        panels.push(Panel {
            title: "peak memory".to_string(),
            bars: mem,
            axis_hi: if mmax > 0.0 { mmax * 1.18 } else { 1.0 },
            fmt: fmt_bytes,
        });
    }
    panels
}

/// The processor and runtime versions stamped on every chart,
/// "Apple M1 Pro 10 cores  node v26.7.0  python 3.14.7". `meta.python` is
/// the full `--version` line, its last word is the number. Results recorded
/// before the core count existed carry a zero, which stays off the label.
fn versions_label(meta: &Meta) -> String {
    let python = meta.python.split_whitespace().last().unwrap_or("?");
    let cores = match meta.cpu_cores {
        0 => String::new(),
        n => format!(" {n} cores"),
    };
    format!("{}{cores}  node {}  python {}", meta.cpu, meta.node, python)
}

fn render_case(out: &Path, c: &CaseResult, meta: &Meta) -> Result<()> {
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

    let title = display_title(&c.name);
    let versions = versions_label(meta);
    let title_style = ("sans-serif", s(30)).into_font().color(&INK);
    let versions_style = ("sans-serif", s(15)).into_font().color(&MUTED);

    // The narrow two panel cases cannot hold the title and the machine line on
    // one row without crowding, so the header stacks when the gap gets too
    // small to read as a separation.
    let title_w = area.estimate_text_size(title, &title_style)?.0;
    let versions_w = area.estimate_text_size(&versions, &versions_style)?.0;
    let header_space = u32::try_from(s(28) * 2 + s(48)).expect("header margins fit u32");
    let one_row = title_w + versions_w + header_space <= dims.0;

    let (head, body) = area.split_vertically(s(if one_row { 60 } else { 82 }));
    if one_row {
        head.draw(&Text::new(title, (s(28), s(18)), title_style))?;
        head.draw(&Text::new(
            versions,
            (s(w - 28), s(28)),
            versions_style.pos(Pos::new(HPos::Right, VPos::Top)),
        ))?;
    } else {
        head.draw(&Text::new(title, (s(28), s(14)), title_style))?;
        head.draw(&Text::new(versions, (s(28), s(56)), versions_style))?;
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
