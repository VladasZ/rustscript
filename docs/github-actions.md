# GitHub Actions

RustScript ships as a GitHub Action. A workflow can install the interpreter
and run scripts without compiling anything.

## Usage

Install only, then use `rust` from any later step in the job.

```yaml
steps:
  - uses: actions/checkout@v5
  - uses: VladasZ/rustscript@v0.2
  - run: rust tools/report.rs
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

`mode` maps onto the CLI. `run` interprets, `build` compiles and runs,
`check` validates without running. `build` and `check` need a cargo
toolchain, which the GitHub hosted images have.

## Outputs

| output | meaning |
| --- | --- |
| `version` | the version that was installed, for example `v0.2.0` |
| `bin-path` | directory holding the installed binary |

## How the version is resolved

The action checks 3 things in order.

1. The `version` input.
2. The tag the action was called with, when it is an exact version like
   `v0.2.0`. So a pinned action installs a matching interpreter.
3. The newest release otherwise. This is what `@v0.2` or `@main` get.

A leading `v` is optional in the input.

## Platforms

| runner | target | archive |
| --- | --- | --- |
| Linux x86_64 | `x86_64-unknown-linux-musl` | `tar.gz` |
| Linux arm64 | `aarch64-unknown-linux-musl` | `tar.gz` |
| macOS | `universal-apple-darwin` | `tar.gz` |
| Windows x86_64 | `x86_64-pc-windows-msvc` | `zip` |
| Windows arm64 | `aarch64-pc-windows-msvc` | `zip` |

The Linux builds are static musl, so they run in any container. `macOS` is
one universal binary. A runner outside this table fails with a clear message.

Every download is checked against the `SHA256SUMS` file of the release. A
repeat install in one job is skipped because the first one lands in the
runner tool cache.

## Cutting a release

Run the Release workflow from the Actions tab and choose `patch`, `minor` or
`major`. In one run it bumps the version in `crates/rustscript/Cargo.toml`,
commits `release vX.Y.Z`, pushes the tag, builds all 5 assets on native
runners, writes `SHA256SUMS`, creates the release and force moves the minor
tag.

It is one workflow on purpose. A tag pushed with the default `GITHUB_TOKEN`
does not trigger other workflows, and splitting it would need a personal
access token.

The workflow does not publish to crates.io on purpose, so the token stays on
the local machine. After the run, pull and run `cargo publish -p run-rs`. A
release is not complete until crates.io has it.

Pushing a tag by hand still works.

```
git tag v0.2.0-rc.1
git push origin v0.2.0-rc.1
```

That is also the only way to cut a prerelease. A tag with a hyphen is marked
as a prerelease, does not move the minor tag and is never picked by a bare
`rust update`. `rust update v0.2.0-rc.1` installs it.

## CI

`ci.yml` runs on every push to `main` and every pull request. It checks
formatting and spelling on `Linux` and runs clippy with `-D warnings` and the
full test suite on `Linux`, `macOS` and `Windows`.

Spelling uses `crate-ci/typos`. Words it does not know, like `ratatui`, go
into `typos.toml`.

Every job sets `timeout-minutes`. No job installs system packages. The bench
crate embeds its font, so no platform needs system font libraries.

## Marketplace

Listing on the Marketplace is a manual step from the release page in the
web interface. Everything it needs is already in `action.yml`.

## Future work

Caching is not implemented yet. The `rust build` binaries and the
`rust check` result cache would be worth caching on top of
[actions/cache](https://github.com/actions/cache) with a `cache` input.
