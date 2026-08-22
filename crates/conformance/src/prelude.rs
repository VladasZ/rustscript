//! re-export hub for `pub use` chains and renames

pub use crate::data::models::Item as ItemAlias;
pub use crate::geometry::shapes::Rect;

pub type Area = i64;
pub const ORIGIN_X: i64 = 2;

pub fn project_name() -> String {
    "multifile".to_string()
}
