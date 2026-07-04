pub mod models;
pub mod store;

/// Re-export, so callers can reach the tag one level up.
pub use self::models::DEFAULT_TAG;
