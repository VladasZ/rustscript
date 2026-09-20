#!/usr/bin/env rust

// An item behind a false `#[cfg(..)]` is not part of the program. The same name can be
// defined once per platform, in any order, and only the one for the host is used.

#[cfg(windows)]
fn family() -> &'static str {
    "windows"
}

#[cfg(not(windows))]
fn family() -> &'static str {
    "not windows"
}

// the host one comes first here and second above, the order must not matter
#[cfg(not(windows))]
const SHELL: &str = "sh";

#[cfg(windows)]
const SHELL: &str = "cmd";

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn known_os() -> bool {
    true
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn known_os() -> bool {
    false
}

#[cfg(all(windows, unix))]
fn never() -> &'static str {
    "no host is both"
}

#[cfg(not(all(windows, unix)))]
fn never() -> &'static str {
    "never loaded the impossible one"
}

fn main() {
    // `cfg!` and `#[cfg]` have to agree
    let family_wanted = if cfg!(windows) {
        "windows"
    } else {
        "not windows"
    };
    println!("family matches: {}", family() == family_wanted);

    let shell_wanted = if cfg!(windows) { "cmd" } else { "sh" };
    println!("shell matches: {}", SHELL == shell_wanted);

    println!("known os: {}", known_os());
    println!("{}", never());
}
