use super::*;

fn output(status: i32, stderr: &str) -> ProcessOutput {
    ProcessOutput {
        status: Some(status),
        stdout: String::new(),
        stderr: stderr.to_string(),
        timed_out: false,
    }
}

#[test]
fn unsupported_errors_are_gaps() {
    assert_eq!(
        classify(&output(0, ""), &output(1, "unsupported item: macro")),
        Classification::InterpreterUnsupported
    );
}

/// Keying on the first stderr line collapses every gap into 1 bucket.
#[test]
fn gaps_bucket_by_reason_not_location() {
    let one = "thread 'main' panicked at case_3.rs:12:\nunknown method `ilog2` on a number\n  at main (case_3.rs:12)\n";
    let two = "thread 'main' panicked at case_3.rs:12:\nunknown method `leading_ones` on a number\n  at main (case_3.rs:12)\n";
    assert_eq!(gap_reason(one), "unknown method `ilog2` on a number");
    assert_eq!(gap_reason(two), "unknown method `leading_ones` on a number");
    assert_eq!(
        gap_reason("rust unsupported: macro `todo`"),
        "rust unsupported: macro `todo`"
    );
}

#[test]
fn different_output_is_a_semantic_failure() {
    let native = ProcessOutput {
        stdout: "one".to_string(),
        ..output(0, "")
    };
    let interpreted = ProcessOutput {
        stdout: "two".to_string(),
        ..output(0, "")
    };
    assert_eq!(
        classify(&native, &interpreted),
        Classification::SemanticMismatch
    );
}

fn panic(payload: &str) -> ProcessOutput {
    ProcessOutput {
        status: Some(PANIC_STATUS),
        stdout: String::new(),
        stderr: format!(
            "thread 'main' panicked at case.rs:1:1:\n{payload}\nnote: run with `RUST_BACKTRACE=1`\n"
        ),
        timed_out: false,
    }
}

#[test]
fn matching_panics_agree_despite_location_and_backtrace_noise() {
    assert_eq!(
        classify(
            &panic("attempt to add with overflow"),
            &panic("attempt to add with overflow")
        ),
        Classification::Match
    );
}

#[test]
fn interpreter_script_backtrace_is_not_part_of_the_message() {
    // the interpreter's `at <frame>` lines must not break agreement
    let native = panic("attempt to multiply with overflow");
    let interpreted = ProcessOutput {
        status: Some(PANIC_STATUS),
        stdout: String::new(),
        stderr: "thread 'main' panicked at case_0.rs:82:\nattempt to multiply with overflow\n  at main (case_0.rs:82)\n".to_string(),
        timed_out: false,
    };
    assert_eq!(classify(&native, &interpreted), Classification::Match);
}

#[test]
fn interpreter_running_past_a_real_panic_is_a_finding() {
    let native = panic("attempt to add with overflow");
    let interpreted = ProcessOutput {
        stdout: "9223372036854775808".to_string(),
        ..output(0, "")
    };
    assert_eq!(
        classify(&native, &interpreted),
        Classification::InterpreterMissingPanic
    );
}

#[test]
fn interpreter_panicking_alone_is_a_finding() {
    assert_eq!(
        classify(&output(0, ""), &panic("attempt to divide by zero")),
        Classification::InterpreterSpuriousPanic
    );
}

#[test]
fn a_runtime_gap_panic_is_a_gap() {
    assert_eq!(
        classify(
            &output(0, ""),
            &panic("unknown method `product` on Iterator")
        ),
        Classification::InterpreterUnsupported
    );
    assert_eq!(
        classify(
            &panic("attempt to add with overflow"),
            &panic("unsupported constant `f64::LOG2_10`")
        ),
        Classification::InterpreterUnsupported
    );
}

#[test]
fn differing_panic_messages_are_a_finding() {
    assert_eq!(
        classify(
            &panic("range end index 5 out of range for slice of length 1"),
            &panic("slice 0..5 out of bounds (len 1)")
        ),
        Classification::PanicMessageMismatch
    );
}

#[test]
fn a_gap_that_hides_a_missing_panic_stays_a_gap() {
    let native = panic("attempt to add with overflow");
    let interpreted = output(1, "unsupported item: macro");
    assert_eq!(
        classify(&native, &interpreted),
        Classification::InterpreterUnsupported
    );
}

#[test]
fn large_captured_output_does_not_block() -> Result<()> {
    let output = run_command(
        Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "runner::tests::large_output_helper",
                "--nocapture",
            ])
            .env("RUSTSCRIPT_TEST_LARGE_OUTPUT", "1"),
        Duration::from_secs(10),
    )?;

    assert!(!output.timed_out);
    assert_eq!(output.status, Some(0));
    assert!(output.stderr.len() >= 1024 * 1024);
    Ok(())
}

#[test]
fn large_output_helper() {
    if std::env::var_os("RUSTSCRIPT_TEST_LARGE_OUTPUT").is_some() {
        eprint!("{}", "x".repeat(1024 * 1024));
    }
}

/// A case with a `crate::` path used to fail the batch build, and every case of the batch
/// then compiled alone.
#[test]
fn a_batch_keeps_the_crate_paths_of_a_case_inside_its_module() -> Result<()> {
    let case = "mod helper {\n    pub fn one() -> u8 {\n        1\n    }\n}\n\nfn main() {\n    println!(\"{}\", crate::helper::one());\n}\n";
    let bundle = render_native_batch(&[case.to_string(), case.to_string()])?;
    assert!(bundle.contains("crate::case_0::helper::one()"));
    assert!(bundle.contains("crate::case_1::helper::one()"));
    assert!(!bundle.contains("crate::helper"));
    Ok(())
}
