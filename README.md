# cpp-infra-ci

[![CI](https://github.com/qubit-ltd/cpp-infra-ci/actions/workflows/ci.yml/badge.svg)](https://github.com/qubit-ltd/cpp-infra-ci/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/qubit-cpp-infra-ci.svg)](https://crates.io/crates/qubit-cpp-infra-ci)
[![Docs](https://docs.rs/qubit-cpp-infra-ci/badge.svg)](https://docs.rs/qubit-cpp-infra-ci)
[![Rust](https://img.shields.io/badge/rust-1.94%2B-orange.svg)](Cargo.toml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![中文](https://img.shields.io/badge/README-简体中文-blue.svg)](README.zh_CN.md)

A C++ library often needs formatting, dependency checks, multiple build profiles, analysis, coverage, and documentation to agree on what passed. `cpp-infra-ci` gives maintainers one task graph and a complete expected-report manifest for local runs and GitHub Actions. A missing platform report fails the gate even when every report that arrived is green.

## Installation

This repository is the source implementation; the badges do not imply a published crate release. Build locally with Rust 1.94:

```bash
cargo install --path . --locked
cpp-infra-ci --version --json
```

For a C++ project with the shared infrastructure installed, use its bootstrap. Each invocation follows the tool repositories' latest `main` revisions and builds or reuses a verified source cache. A workflow resolves the source group once and shares the portable revisions file across its jobs; public project configuration changes only through explicit updates.

## Quick start

Suppose a library already has `.infra/project.toml` with a `debug` profile and configured style/verify tools. Create `.infra/ci/config.toml`:

```toml
schema = 1
tasks = ["style", "verify"]
primary_profile = "debug"
local_fail_fast = true
task_jobs = 1
matrix_max_parallel = 4

[[matrix]]
id = "linux-debug"
runner = "ubuntu-latest"
os = "linux"
arch = "x86_64"
profile = "debug"
suites = ["build", "test", "headers", "install"]
```

Inspect the commands, then execute the same graph:

```bash
python3 .infra/bootstrap.py run --tool cpp-infra-ci -- plan
python3 .infra/bootstrap.py run --tool cpp-infra-ci -- check
python3 .infra/bootstrap.py run --tool cpp-infra-ci -- check --only style
```

`plan` launches no checking tools and emits no pretend gate results. `check` launches the selected executables, validates their public reports and artifact hashes, and writes `quality-summary.json`, then optional Pages results, then `final-summary.json` beneath `build/infra-runs/<run-id>/`. Inspect the final report's `status`, `data.complete`, and `data.incomplete_remote_coverage`. Non-host matrix entries are explicitly listed as `remote_only`; set `require_complete_matrix = true` if a local partial platform run must fail.

A requested task needs its configuration. `dependency` needs the primary profile's `conan_context`; `analyze` and `coverage` use the `profile` in their own configuration, defaulting to `analysis` and `coverage`. Their verify preparation is performed before the check, and validated build contexts are forwarded for reuse. Unknown selections, duplicate tasks, dangling profiles, unsupported schemas, path escapes, and dependency cycles fail before execution. Configuration files, declared project roots, hooks, and their project-local parents use ordinary paths; symbolic links are rejected so changing a selector cannot bypass input fingerprints.

## GitHub Actions

A consumer can call the reusable workflow, which tracks `main` as requested by the tool distribution model:

```yaml
name: C++ checks
on: [push, pull_request]
jobs:
  cpp:
    uses: qubit-ltd/cpp-infra-ci/.github/workflows/cpp-ci.yml@main
    permissions:
      contents: read
```

The delivered [workflow](.github/workflows/cpp-ci.yml) resolves one portable source group, plans the graph, runs light checks and build/analysis branches, gathers an always-running quality summary, builds Pages when enabled, and produces an always-running final summary. Deployment requires `publish_pages: true`, a successful final gate, a non-PR event, and the default branch. Only the deployment job requests Pages write permissions. External actions use fixed commit SHAs. Runner labels select scheduling environments; configured external tools still have to be available and satisfy `tools doctor`.

For a custom runner adapter:

```bash
python3 .infra/bootstrap.py resolve --output build/ci/revisions.json
export CPP_INFRA_REVISIONS_FILE="$PWD/build/ci/revisions.json"
python3 .infra/bootstrap.py run --tool cpp-infra-ci -- matrix plan --output build/ci/matrix.json
python3 .infra/bootstrap.py run --tool cpp-infra-ci -- execute \
  --manifest build/ci/run-manifest.json --task-ids doctor-style,style \
  --output-dir build/infra-runs/SHARED-RUN-ID
```

Use the actual `plan.run_id` instead of `SHARED-RUN-ID`. `matrix plan` assigns each logical node to exactly one job group. Entries with the same runner, OS, architecture, and profile share one preparation and the union of their suites; different environments retain separate doctor, preparation, and suite evidence. `execute` writes only to the canonical run directory and refuses unmanaged existing run files. It reuses verified prerequisite reports; `--require-existing-dependencies` prevents a job from recreating absent upstream tasks. `export --input-dir ... --task-ids ... --output-dir ...` copies owned public reports, raw child reports, logs, and recursively referenced artifacts without changing their identity. `--allow-missing` preserves available diagnostics after failures; final summary still rejects missing required evidence.

## Reports and failures

```bash
cpp-infra-ci --project /path/to/library summarize --input-dir /path/to/library/build/infra-runs/RUN-ID
cpp-infra-ci --project /path/to/library matrix run --check linux-debug
```

Reports outside `build` can use a new non-input JSON file or an existing public tool report. Validated `source_excluded_paths` travel with the run so a generated report never becomes a child tool’s source input; protected inputs and existing business JSON cannot be overwritten.

Relative `--manifest` paths are resolved from `--project`. A manifest stored separately from downloaded reports retains its canonical path and the SHA256 of its verified bytes in `data.manifest_provenance`; it is referenced as an artifact only when it is inside the report root.

Summaries require the original `run-manifest.json`, verify the CI producer version/revision, and require the original authenticated child report for every passed external node. Pages upload discovers the `site` artifact from that validated report, including its private call directory. They reject duplicate task IDs, unknown bundles, source/configuration mismatches, different matrix definitions, and changed required artifacts. Artifact paths are relative to the downloaded report directory, so reports can move between runners without rewriting their original absolute project path. Configured hooks use `[[hooks]]` with `id`, project-relative `executable`, `args`, `needs`, `timeout_seconds`, and `required`; shell text is never interpreted.

`task_jobs` controls concurrent task processes, independently of build-tool parallelism. Shared profile/configure resources are serialized, while child tools retain their own build locks. Local fail-fast leaves unstarted tasks `blocked`. Timeouts and SIGINT stop child process groups on Unix; all outcomes retain report evidence. Exit codes are 0 success, 1 gate failure, 2 configuration/protocol error, 3 filesystem/process infrastructure error, and 130 cancellation.

## Testing

```bash
cargo +nightly-2026-06-05 fmt --all -- --config-path .infra/style/rustfmt.toml --check
cargo test --locked
cargo +nightly-2026-06-05 clippy --locked --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps
```

Tests use self-contained projects and controlled executable fixtures. They verify actual process scheduling and report validation; they do not run downstream repositories. Real CMake, analysis, coverage, dependency, and Pages acceptance belongs to the cross-tool integration run.

## License

[Apache License 2.0](LICENSE).

## Contributing

Keep changes scoped to the affected tool contract and add focused behavior tests. Do not use downstream all-features CI as this crate's test suite.

## Author

Maintained in the [Qubit organization](https://github.com/qubit-ltd).
