//! parent module in `name.rs` style

pub mod ops;
pub mod shapes;

// a `#[path]` in a `name.rs` file resolves from that file's dir, not from `geometry/`
#[path = "units.rs"]
pub mod units;
