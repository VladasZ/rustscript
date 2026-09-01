const GIT_COMMIT: &str = env!("RUSTSCRIPT_GIT_COMMIT");
const BUILD_TIME: &str = env!("RUSTSCRIPT_BUILD_TIME");
const BUILD_PROFILE: &str = env!("RUSTSCRIPT_BUILD_PROFILE");

fn short_commit(commit: &str) -> String {
    let short = commit.get(..7).unwrap_or(commit);
    if commit.ends_with("-dirty") {
        format!("{short}-dirty")
    } else {
        short.to_string()
    }
}

pub fn version() -> String {
    format!(
        "rustscript {} ({}, built {}, {})",
        env!("CARGO_PKG_VERSION"),
        short_commit(GIT_COMMIT),
        BUILD_TIME,
        BUILD_PROFILE
    )
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct ToolchainFile {
        toolchain: Toolchain,
    }

    #[derive(Deserialize)]
    struct Toolchain {
        channel: String,
    }

    /// `cargo install run-rs` checks `rust-version` before it builds, so an old toolchain gets a
    /// clear message instead of a compile error. Only the pinned toolchain is proven by CI, so
    /// the claim must move with it.
    #[test]
    fn rust_version_matches_the_pinned_toolchain() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../rust-toolchain.toml");
        let text = std::fs::read_to_string(path).unwrap();
        let file: ToolchainFile = toml::from_str(&text).unwrap();
        let (pinned, _) = file
            .toolchain
            .channel
            .rsplit_once('.')
            .expect("the channel is a full X.Y.Z version");
        assert_eq!(env!("CARGO_PKG_RUST_VERSION"), pinned);
    }
}
