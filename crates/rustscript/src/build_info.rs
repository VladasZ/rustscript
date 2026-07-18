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
