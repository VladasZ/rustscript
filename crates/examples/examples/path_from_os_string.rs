// `PathBuf::from` of an `OsString` must unwrap the inner path, and
// `UNIX_EPOCH` must be a `SystemTime`. Only booleans and a zero delta are
// printed.

use std::env;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    // PATH is always set.
    if let Some(v) = env::var_os("PATH") {
        let p = PathBuf::from(v);
        // Once printed as "OsString { s: .. }".
        println!(
            "has_debug_wrapper: {}",
            p.display().to_string().contains("OsString {")
        );
    }

    let zero = UNIX_EPOCH.duration_since(UNIX_EPOCH).unwrap().as_millis();
    println!("epoch_delta: {zero}");

    let now_after_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        > 0;
    println!("now_after_epoch: {now_after_epoch}");
}
