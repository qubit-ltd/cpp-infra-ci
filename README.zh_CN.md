# cpp-infra-ci

[![CI](https://github.com/qubit-ltd/cpp-infra-ci/actions/workflows/ci.yml/badge.svg)](https://github.com/qubit-ltd/cpp-infra-ci/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/qubit-cpp-infra-ci.svg)](https://crates.io/crates/qubit-cpp-infra-ci)
[![Docs](https://docs.rs/qubit-cpp-infra-ci/badge.svg)](https://docs.rs/qubit-cpp-infra-ci)
[![Rust](https://img.shields.io/badge/rust-1.94%2B-orange.svg)](Cargo.toml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![English](https://img.shields.io/badge/README-English-blue.svg)](README.md)

维护 C++ 库时，格式、依赖、不同构建配置、静态分析、覆盖率和文档往往各有入口，最终还要确认结果是否齐全。`cpp-infra-ci` 把这些步骤组织成同一张任务依赖图，供本地和 GitHub Actions 使用。汇总依据规划时确定的完整报告清单；即使收到的报告全绿，只要缺少必需的平台结果，门禁仍会失败。

## 安装

当前仓库提供源码实现；徽章不代表 crate 已经发布。使用 Rust 1.94 构建：

```bash
cargo install --path . --locked
cpp-infra-ci --version --json
```

C++ 项目安装公共基础设施后，通过项目的 bootstrap 使用工具。入口每次跟踪工具仓库 `main` 的最新 revision，并从源码构建或复用经过校验的缓存。GitHub 工作流只解析一次源码组，再让各 job 共用可移植的 revision 文件；项目公共配置仍需显式更新。

## 快速开始

假设一个库已经在 `.infra/project.toml` 中定义 `debug` profile，并配置 style 和 verify 工具。在 `.infra/ci/config.toml` 中声明检查：

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

先查看计划，再运行同一套检查；需要时可只选其中一部分：

```bash
python3 .infra/bootstrap.py run --tool cpp-infra-ci -- plan
python3 .infra/bootstrap.py run --tool cpp-infra-ci -- check
python3 .infra/bootstrap.py run --tool cpp-infra-ci -- check --only style
```

`plan` 不调用检查工具，也不生成冒充检查结果的报告。`check` 实际启动工具，校验公共报告及产物摘要，在 `build/infra-runs/<run-id>/` 依次生成 `quality-summary.json`、可选 Pages 结果及 `final-summary.json`。查看最终报告的 `status`、`data.complete` 和 `data.incomplete_remote_coverage`，即可判断门禁是否通过、证据是否齐全、是否仍缺远端平台验收。本机不适用的矩阵项会列入 `remote_only`；若本地缺少完整平台结果也必须失败，可配置 `require_complete_matrix = true`。

显式选择的任务必须有对应配置。`dependency` 需要主 profile 的 `conan_context`；`analyze`、`coverage` 使用各自配置中的 `profile`，默认分别为 `analysis`、`coverage`。CI 会先执行相应的 verify 准备步骤，再把经过验证的构建上下文传给检查工具复用。未知或重复任务、悬空 profile、不支持的 schema、路径越界和依赖环均会在执行前被拒绝。配置文件、声明的项目根、hook 及其项目内祖先必须使用普通路径；符号链接会被拒绝，避免切换入口后绕过输入指纹。

## GitHub Actions

消费项目可引用交付的复用工作流，按工具分发模型跟踪 `main`：

```yaml
name: C++ checks
on: [push, pull_request]
jobs:
  cpp:
    uses: qubit-ltd/cpp-infra-ci/.github/workflows/cpp-ci.yml@main
    permissions:
      contents: read
```

[复用工作流](.github/workflows/cpp-ci.yml) 先固定本次源码组并规划任务，再运行轻量检查、构建矩阵和分析分支。随后，无论检查是否失败，都尽量汇集质量报告；启用 Pages 时构建站点，最后再次汇总整条流水线。部署须同时满足 `publish_pages: true`、最终门禁成功、非 PR 事件和默认分支条件。只有部署 job 请求 Pages 写权限，外部 actions 均固定提交 SHA。runner 标签只影响调度，环境仍须具备项目配置的外部工具，并通过 `tools doctor` 校验。

自定义 runner 适配器可以直接使用同一份任务清单：

```bash
python3 .infra/bootstrap.py resolve --output build/ci/revisions.json
export CPP_INFRA_REVISIONS_FILE="$PWD/build/ci/revisions.json"
python3 .infra/bootstrap.py run --tool cpp-infra-ci -- matrix plan --output build/ci/matrix.json
python3 .infra/bootstrap.py run --tool cpp-infra-ci -- execute \
  --manifest build/ci/run-manifest.json --task-ids doctor-style,style \
  --output-dir build/infra-runs/SHARED-RUN-ID
```

将 `SHARED-RUN-ID` 换成 `plan.run_id`。`matrix plan` 为各 job 分配互不重复的逻辑任务。runner、操作系统、架构和 profile 都相同的项会合并 suite，共用一次准备；环境不同的项各自保留 doctor、准备和 suite 证据。`execute` 仅写入规范运行目录，拒绝覆盖不属于该运行的已有文件，并复用经过验证的前置报告；加上 `--require-existing-dependencies` 后，缺少上游证据会直接报错，不会在当前 job 重新创建前置任务。`export --input-dir ... --task-ids ... --output-dir ...` 只导出本 job 拥有的公共报告、子工具原始报告、日志和递归引用的产物，不改写原始身份。失败后可用 `--allow-missing` 保存已有诊断，最终汇总仍会拒绝缺失的必需证据。

## 报告与失败处理

```bash
cpp-infra-ci --project /path/to/library summarize --input-dir /path/to/library/build/infra-runs/RUN-ID
cpp-infra-ci --project /path/to/library matrix run --check linux-debug
```

`build` 之外也可以把报告写入新建的非输入 JSON 文件，或覆盖已有公共工具报告。经校验的 `source_excluded_paths` 会随整次运行传递，生成报告不会反过来成为子工具的源码输入；受保护输入和已有业务 JSON 均不能被覆盖。

`--manifest` 的相对路径以 `--project` 为基准。清单与下载报告分开存放时，`data.manifest_provenance` 保留其规范路径和已验证原始字节的 SHA256；只有清单位于报告根内时，才将其列为相对 artifact。

汇总必须提供原始 `run-manifest.json`，校验 CI 生产者的版本与 revision，并要求每个通过的外部节点保留身份匹配的原始子报告。Pages 上传从已验证报告的 `site` 产物定位实际目录，包括工具私有调用子目录。重复 task ID、未知 bundle、源码或配置不匹配、矩阵定义不同以及必需产物被修改，都会导致失败。跨 runner 下载后，产物按报告目录内的相对路径定位，无须改写原报告中的绝对项目路径。项目 hook 使用 `[[hooks]]`，字段为 `id`、项目内的 `executable`、`args`、`needs`、`timeout_seconds` 和 `required`，不会解释 shell 文本。

`task_jobs` 控制任务进程并发，与构建工具内部的并发预算独立。共享 profile 或 configure 资源的任务会串行调度，子工具仍保留自己的构建锁。本地 fail-fast 会把未启动任务记为 `blocked`。Unix 下超时和 SIGINT 会停止子进程组并保留报告。退出码分别为：0 成功，1 门禁失败，2 配置或协议错误，3 文件系统或进程基础设施错误，130 取消。

## 测试

```bash
cargo +nightly-2026-06-05 fmt --all -- --config-path .infra/style/rustfmt.toml --check
cargo test --locked
cargo +nightly-2026-06-05 clippy --locked --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps
```

测试使用自带项目和受控可执行程序，实际验证调度、进程结果和报告完整性，不运行真实下游仓库。真实 CMake、分析、覆盖率、依赖和 Pages 检查由跨工具集成验收统一执行。

## 许可证

采用 [Apache License 2.0](LICENSE)。

## 贡献

请将修改限定在相关工具契约内，并补充有意义的行为测试。本 crate 的 CI 不加入 downstream all-features 检查。

## 作者

由 [Qubit 组织](https://github.com/qubit-ltd)维护。
