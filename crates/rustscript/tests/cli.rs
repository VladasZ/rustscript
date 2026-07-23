use std::process::Command;

use pretty_assertions::assert_eq;

fn rust(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_rust"))
        .args(args)
        .output()
        .expect("failed to launch rustscript");
    assert!(
        output.status.success(),
        "rustscript failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("rustscript returned non-UTF-8 output")
}

#[test]
fn version_contains_build_metadata() {
    let version = rust(&["--version"]);
    assert!(version.starts_with(concat!("rustscript ", env!("CARGO_PKG_VERSION"), " (")));
    assert!(version.contains(", built "));
    let profile = version
        .trim_end()
        .rsplit_once(", ")
        .and_then(|(_, tail)| tail.strip_suffix(')'))
        .expect("version should end with a build profile");
    assert!(!profile.is_empty());
    assert_eq!(rust(&["-V"]), version);
}

#[test]
fn help_lists_update_and_version() {
    let help = rust(&["help"]);
    assert!(help.contains("rust update"));
    assert!(help.contains("rust --version"));
    assert!(help.contains("rust -e"));
}

#[test]
fn eval_runs_a_statement_snippet() {
    assert_eq!(
        rust(&["-e", r#"println!("hi from eval")"#]),
        "hi from eval\n"
    );
}

#[test]
fn eval_passes_arguments_and_names_argv0_dash_e() {
    let out = rust(&[
        "-e",
        r#"let args: Vec<String> = std::env::args().collect(); println!("{}|{}|{}", args[0], args[1], args[2]);"#,
        "one",
        "two",
    ]);
    assert_eq!(out, "-e|one|two\n");
}

#[test]
fn eval_runs_a_complete_program_with_main() {
    let out = rust(&["-e", r#"fn main() { println!("full program"); }"#]);
    assert_eq!(out, "full program\n");
}

#[test]
fn eval_supports_question_mark() {
    let out = Command::new(env!("CARGO_BIN_EXE_rust"))
        .args([
            "-e",
            r#"std::fs::read_to_string("definitely_missing_file_for_eval_test")?;"#,
        ])
        .output()
        .expect("failed to launch rustscript");
    assert!(!output_success(&out), "missing file must fail through `?`");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Error"), "stderr was: {stderr}");
}

#[test]
fn eval_snippet_ending_in_comment_still_parses() {
    let out = rust(&["-e", "println!(\"ok\"); // trailing comment"]);
    assert_eq!(out, "ok\n");
}

fn output_success(out: &std::process::Output) -> bool {
    out.status.success()
}
