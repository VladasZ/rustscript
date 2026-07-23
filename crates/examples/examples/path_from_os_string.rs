// Covers two std bridges the board tooling leans on: PathBuf::from of an OsString (from env::var_os)
// must unwrap the inner path rather than Debug-print the struct, and std::time::UNIX_EPOCH must resolve
// as a SystemTime so duration_since math works. Output is deterministic: it prints only booleans and a
// zero delta, never the machine-specific path or the wall clock, so compiled and interpreted match.

use std::env;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    // PATH is always set, and both runs share this machine's environment, so the value is identical.
    if let Some(v) = env::var_os("PATH") {
        let p = PathBuf::from(v);
        // After the fix the display is the real path text, never the "OsString { s: .. }" debug form.
        println!("has_debug_wrapper: {}", p.display().to_string().contains("OsString {"));
    }

    let zero = UNIX_EPOCH.duration_since(UNIX_EPOCH).unwrap().as_millis();
    println!("epoch_delta: {zero}");

    let now_after_epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() > 0;
    println!("now_after_epoch: {now_after_epoch}");
}
