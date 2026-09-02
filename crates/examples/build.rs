//! Windows links a 1 MiB main-thread stack while Unix gives 8 MiB, so the deep recursion
//! examples overflow only there. Raise the link-time stack reserve to match Unix.

use std::env::var;

fn main() {
    if var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let arg = if var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        "/STACK:8388608"
    } else {
        "-Wl,--stack,8388608"
    };
    println!("cargo:rustc-link-arg-examples={arg}");
}
