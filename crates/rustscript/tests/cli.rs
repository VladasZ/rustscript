use std::process::Command;

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
}
