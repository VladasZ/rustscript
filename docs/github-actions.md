# GitHub Actions

RustScript ships as a GitHub Action, so a workflow can install the interpreter
and run scripts with it without compiling anything.

## Usage

Install only, then use `rust` from any later step in the job.

```yaml
steps:
  - uses: actions/checkout@v5
  - uses: VladasZ/rustscript@v0.2
  - run: rust run tools/report.rs
```

Install and run in one step.

```yaml
steps:
  - uses: actions/checkout@v5
  - uses: VladasZ/rustscript@v0.2
    with:
      script: tools/release.rs
      args: --dry-run
```

Pin the interpreter version independently of the action.

```yaml
  - uses: VladasZ/rustscript@v0.2
    with:
      version: v0.2.0
```

## Inputs

| input | default | meaning |
| --- | --- | --- |
| `version` | the calling tag, else newest | version to install, for example `v0.2.0` |
| `script` | empty | script to execute, empty means install only |
| `mode` | `run` | `run`, `build` or `check` |
| `args` | empty | extra arguments passed to the script |
| `github-token` | `github.token` | only used to resolve the newest release |

`mode` maps straight onto the CLI. `run` interprets, `build` compiles with cargo
and then runs, and `check` validates with `cargo check` and the interpreter
coverage walk without running. Both
`build` and `check` need a cargo toolchain in the job, which the GitHub hosted
images already provide.

## Outputs

| output | meaning |
| --- | --- |
| `version` | the version that was installed, for example `v0.2.0` |
| `bin-path` | directory holding the installed binary |

## How the version is resolved

The action checks three things in order.

1. The `version` input, when it is set.
2. The tag the action itself was called with, when that tag is an exact version
   like `v0.2.0`. This is what makes a pinned action install a matching
   interpreter with no extra configuration.
3. The newest release otherwise, which is what a moving tag like `@v0.2` or a
   branch like `@main` falls back to.

A leading `v` is optional in the input, so `0.2.0` and `v0.2.0` both work.

## Platforms

| runner | target | archive |
| --- | --- | --- |
| Linux x86_64 | `x86_64-unknown-linux-musl` | `tar.gz` |
| Linux arm64 | `aarch64-unknown-linux-musl` | `tar.gz` |
| macOS | `universal-apple-darwin` | `tar.gz` |
| Windows x86_64 | `x86_64-pc-windows-msvc` | `zip` |
| Windows arm64 | `aarch64-pc-windows-msvc` | `zip` |

The Linux builds are static musl binaries, so they need no particular glibc and
run in any container, Alpine included. macOS is a single universal binary, so
Intel and Apple Silicon share one asset. A runner outside this table fails with
a clear message rather than downloading something wrong.

Every download is checked against the `SHA256SUMS` file published with the
release. A mismatch fails the step, so a truncated or tampered download cannot
surface later as a confusing interpreter error.

Within one job a repeat install is skipped, because the first one unpacks into a
versioned directory under the runner tool cache and the second finds it there.

## Cutting a release

Releases start from the Actions tab. Open the Release workflow, press Run
workflow, and choose `patch`, `minor` or `major`. There is no version to
remember to bump.

The workflow then does all of this in one run.

1. Works out the next version from the current one in
   `crates/rustscript/Cargo.toml`.
2. Stops if that tag already exists on the remote.
3. Rewrites the package version, syncs `Cargo.lock`, and commits it as
   `release v0.2.5`.
4. Pushes the commit and the tag.
5. Checks the tag matches the crate version, so assets can never disagree with
   the tag they ship under.
6. Builds all five assets on native runners, so nothing is cross compiled.
7. Writes `SHA256SUMS` over the assets.
8. Creates the release with generated notes.
9. Force moves the minor tag, so `v0.2` follows the newest `v0.2.z`.

It is one workflow rather than a bump workflow feeding a release workflow on
purpose. A tag pushed with the default `GITHUB_TOKEN` does not trigger other
workflows, since GitHub blocks that cascade to prevent loops. Keeping it in one
run avoids needing a personal access token just to wire two halves together.

The workflow does not publish to crates.io on purpose, so the registry token
never leaves the local machine. After the run finishes, pull the release commit
and publish with `cargo publish -p run-rs`. A release is not complete until
crates.io has it.

Pushing a tag by hand still works and skips only the bump.

```
git tag v0.2.0-rc.1
git push origin v0.2.0-rc.1
```

That is also the only way to cut a prerelease, since the dispatch form offers
just the three ordinary bumps. A tag with a hyphen in it is marked as a
prerelease on the release, does not move the minor tag, and is ignored by
`rust update`.

## CI

`ci.yml` runs on every push to `main` and every pull request. It checks
formatting and clippy on Linux, and runs the full test suite on Linux, macOS and
Windows. Clippy runs with `-D warnings`, so a warning fails the build.

## Marketplace

Listing on the GitHub Marketplace is a manual step. It needs the repository to
be public, and it is done from the release page in the GitHub web interface
after accepting the developer agreement. Everything the listing needs, including
the `branding` block, is already in `action.yml`.

## Future work

Caching is not implemented yet. Two things would be worth caching.

- The compiled script binaries that `rust build` and `cmp` mode produce, keyed
  by source hash, so a CPU heavy script pays its cargo build once across runs
  rather than once per run.
- The `rust check` result cache, for the same reason.

Both would sit on top of [actions/cache](https://github.com/actions/cache) and
would add a `cache` input to the action.
