use std::env;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    fn below(&mut self, bound: u64) -> u64 {
        (self.next() >> 33) % bound
    }
}

fn words(tokens: u64) -> Result<String> {
    let vocab = 400u64;
    let mut rng = Lcg(0x1234_5678_9abc_def0);
    let mut text = String::new();
    for i in 0..tokens {
        let index = rng.below(vocab) * rng.below(vocab) / vocab;
        write!(text, "w{index:04}")?;
        if (i + 1) % 12 == 0 {
            text.push('\n');
        } else {
            text.push(' ');
        }
    }
    if !text.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

fn json(records: u64) -> Result<String> {
    let mut rng = Lcg(0x0fed_cba9_8765_4321);
    let mut json = String::from("[");
    for i in 0..records {
        if i > 0 {
            json.push(',');
        }
        let value = rng.below(1_000);
        write!(json, r#"{{"id":{i},"value":{value}}}"#)?;
    }
    json.push(']');
    Ok(json)
}

fn big_script(dir: &Path) -> Result<()> {
    let functions = 300u64;
    let mut rng = Lcg(0x5eed_5eed_5eed_5eed);
    let adds: Vec<u64> = (0..functions).map(|_| rng.below(99_991)).collect();

    let mut rust = String::from("use std::time::Instant;\n\n");
    let mut typescript = String::new();
    let mut python = String::from("import sys\nimport time\n\n");
    for (i, add) in adds.iter().enumerate() {
        write!(
            rust,
            "fn f{i:03}(x: i64) -> i64 {{\n    (x + {add}) % 99991\n}}\n\n"
        )?;
        write!(
            typescript,
            "function f{i:03}(x: number): number {{\n  return (x + {add}) % 99991;\n}}\n\n"
        )?;
        write!(
            python,
            "def f{i:03}(x):\n    return (x + {add}) % 99991\n\n\n"
        )?;
    }

    rust.push_str("fn main() {\n    let t = Instant::now();\n    let mut acc: i64 = 1;\n");
    typescript.push_str("const t = performance.now();\nlet acc = 1;\n");
    python.push_str("t = time.perf_counter_ns()\nacc = 1\n");
    for i in 0..functions {
        writeln!(rust, "    acc = f{i:03}(acc);")?;
        writeln!(typescript, "acc = f{i:03}(acc);")?;
        writeln!(python, "acc = f{i:03}(acc)")?;
    }
    rust.push_str(
        r#"    let ns = t.elapsed().as_nanos();
    println!("acc = {acc}");
    eprintln!("COMPUTE_NS {ns}");
}
"#,
    );
    typescript.push_str(
        r#"const ns = Math.round((performance.now() - t) * 1e6);
console.log(`acc = ${acc}`);
console.error(`COMPUTE_NS ${ns}`);
"#,
    );
    python.push_str(
        r#"ns = time.perf_counter_ns() - t
print(f"acc = {acc}")
print(f"COMPUTE_NS {ns}", file=sys.stderr)
"#,
    );

    fs::create_dir_all(dir)?;
    fs::write(dir.join("case.rs"), rust)?;
    fs::write(dir.join("case.ts"), typescript)?;
    fs::write(dir.join("case.py"), python)?;
    Ok(())
}

fn multifile_script(dir: &Path) -> Result<()> {
    let module_count = 30;
    let functions_per_module = 10;
    let modules = dir.join("modules");
    if modules.exists() {
        fs::remove_dir_all(&modules)?;
    }
    fs::create_dir_all(&modules)?;

    let mut rng = Lcg(0xa11c_e55e_1234_9876);
    let mut rust_index = String::new();
    let mut typescript_index = String::new();
    let mut python_index = String::new();
    let mut rust_run = String::from("pub fn run(mut x: i64) -> i64 {\n");
    let mut typescript_run = String::from("export function run(x: number): number {\n");
    let mut python_run = String::from("def run(x):\n");

    for module in 0..module_count {
        writeln!(rust_index, "pub mod m{module:03};")?;
        writeln!(
            typescript_index,
            "import * as m{module:03} from \"./m{module:03}.ts\";"
        )?;
        writeln!(python_index, "from . import m{module:03}")?;
        writeln!(rust_run, "    x = m{module:03}::apply(x);")?;
        writeln!(typescript_run, "  x = m{module:03}.apply(x);")?;
        writeln!(python_run, "    x = m{module:03}.apply(x)")?;

        let mut rust_module = String::new();
        let mut typescript_module = String::new();
        let mut python_module = String::new();
        for function in 0..functions_per_module {
            let add = rng.below(99_991);
            writeln!(
                rust_module,
                "fn f{function:02}(x: i64) -> i64 {{\n    (x + {add}) % 99991\n}}"
            )?;
            writeln!(
                typescript_module,
                "function f{function:02}(x: number): number {{\n  return (x + {add}) % 99991;\n}}"
            )?;
            writeln!(
                python_module,
                "def f{function:02}(x):\n    return (x + {add}) % 99991\n"
            )?;
        }
        rust_module.push_str("\npub fn apply(mut x: i64) -> i64 {\n");
        typescript_module.push_str("\nexport function apply(x: number): number {\n");
        python_module.push_str("\ndef apply(x):\n");
        for function in 0..functions_per_module {
            writeln!(rust_module, "    x = f{function:02}(x);")?;
            writeln!(typescript_module, "  x = f{function:02}(x);")?;
            writeln!(python_module, "    x = f{function:02}(x)")?;
        }
        rust_module.push_str("    x\n}\n");
        typescript_module.push_str("  return x;\n}\n");
        python_module.push_str("    return x\n");
        fs::write(modules.join(format!("m{module:03}.rs")), rust_module)?;
        fs::write(modules.join(format!("m{module:03}.ts")), typescript_module)?;
        fs::write(modules.join(format!("m{module:03}.py")), python_module)?;
    }

    rust_run.push_str("    x\n}\n");
    typescript_run.push_str("  return x;\n}\n");
    python_run.push_str("    return x\n");
    rust_index.push('\n');
    rust_index.push_str(&rust_run);
    typescript_index.push('\n');
    typescript_index.push_str(&typescript_run);
    python_index.push('\n');
    python_index.push_str(&python_run);

    fs::write(modules.join("mod.rs"), rust_index)?;
    fs::write(modules.join("index.ts"), typescript_index)?;
    fs::write(modules.join("__init__.py"), python_index)?;
    fs::write(
        dir.join("case.rs"),
        "mod modules;\n\nfn main() {\n    println!(\"acc = {}\", modules::run(1));\n}\n",
    )?;
    fs::write(
        dir.join("case.ts"),
        "import { run } from \"./modules/index.ts\";\n\nconsole.log(`acc = ${run(1)}`);\n",
    )?;
    fs::write(
        dir.join("case.py"),
        "from modules import run\n\nprint(f\"acc = {run(1)}\")\n",
    )?;
    Ok(())
}

fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = env::args()
        .nth(1)
        .map(PathBuf::from)
        .context("pass an isolated scratch directory for large fixtures")?;
    let word_scratch = scratch.join("word_count");
    let json_scratch = scratch.join("json");
    fs::create_dir_all(&word_scratch)?;
    fs::create_dir_all(&json_scratch)?;
    let base_text = words(250_000)?;
    let big_text = words(2_500_000)?;
    let base_json = json(200_000)?;
    let big_json = json(2_000_000)?;
    fs::write(root.join("cases/word_count/data.txt"), &base_text)?;
    fs::write(word_scratch.join("data_big.txt"), &big_text)?;
    fs::write(root.join("cases/json/data.json"), &base_json)?;
    fs::write(json_scratch.join("data_big.json"), &big_json)?;
    fs::write(
        root.join("cases/automation/config.json"),
        r#"{"pattern":"w0\\d\\d","top":20}"#,
    )?;
    big_script(&root.join("cases/big_script"))?;
    multifile_script(&root.join("cases/multifile_startup"))?;
    println!(
        "generated text {} / {} bytes and json {} / {} bytes",
        base_text.len(),
        big_text.len(),
        base_json.len(),
        big_json.len()
    );
    Ok(())
}
