// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Command-line adaptation and distributed report aggregation.
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use clap::Parser;
use clap::Subcommand;
use serde_json::Value;
use serde_json::json;

use crate::config;
use crate::digest;
use crate::error::Error;
use crate::plan::Plan;
use crate::planner;
use crate::report;
use crate::scheduler;

#[derive(Parser)]
#[command(
    name = "cpp-infra-ci",
    about = "Plan and execute reproducible C++ quality gates",
    disable_version_flag = true
)]
struct Cli {
    #[arg(long, global = true, default_value = ".")]
    project: PathBuf,
    #[arg(long, global = true)]
    report: Option<PathBuf>,
    #[arg(long, global = true)]
    offline: bool,
    #[arg(long, global = true)]
    run_context: Option<PathBuf>,
    #[arg(long, global = true)]
    version: bool,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<Operation>,
}
#[derive(Subcommand)]
enum Operation {
    /// Print the deterministic local plan without launching tools.
    Plan {
        #[arg(long)]
        only: Option<String>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Execute local quality checks and both summary stages.
    Check {
        #[arg(long)]
        only: Option<String>,
    },
    /// Produce or execute the shared CI matrix.
    Matrix {
        #[command(subcommand)]
        command: MatrixOperation,
    },
    /// Locate a verified raw-tool artifact without assuming its private call
    /// directory.
    ArtifactPath {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        input_dir: PathBuf,
        #[arg(long)]
        task_id: String,
        #[arg(long)]
        kind: String,
    },
    /// Export only owned task reports, logs, and recursively referenced
    /// artifacts.
    Export {
        #[arg(long)]
        input_dir: PathBuf,
        #[arg(long)]
        task_ids: String,
        #[arg(long)]
        output_dir: PathBuf,
        #[arg(long)]
        allow_missing: bool,
    },
    /// Execute an exact subset of a shared manifest, reusing verified
    /// prerequisites.
    Execute {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        task_ids: String,
        #[arg(long)]
        output_dir: PathBuf,
        #[arg(long)]
        require_existing_dependencies: bool,
    },
    /// Verify all expected reports using the original planning manifest.
    Summarize {
        #[arg(long)]
        input_dir: PathBuf,
        #[arg(long)]
        manifest: Option<PathBuf>,
        #[arg(long,default_value="final",value_parser=["quality","final"])]
        phase: String,
    },
}
#[derive(Subcommand)]
enum MatrixOperation {
    Plan {
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Run {
        #[arg(long)]
        check: String,
    },
}

/// Parses CLI arguments, performs requested IO, and returns the protocol exit
/// code.
///
/// Configuration and report identity violations return `Error::Config`;
/// filesystem and child launch failures retain `Error::Io`. Gate failures
/// return exit code 1.
pub fn run() -> Result<i32, Error> {
    let cli = Cli::parse();
    if cli.version {
        if cli.json {
            println!(
                "{}",
                json!({"name":"cpp-infra-ci","version":env!("CARGO_PKG_VERSION"),"revision":option_env!("CPP_INFRA_BUILD_REVISION"),"protocol":1})
            );
        } else {
            println!("cpp-infra-ci {}", env!("CARGO_PKG_VERSION"));
        }
        return Ok(0);
    }
    crate::cancellation::install()?;
    let root = cli.project.canonicalize()?;
    let result = (|| {
        let context = cli.run_context.as_ref().map(|p| report::read(p)).transpose()?;
        crate::report_paths::initialize(&root, cli.report.as_deref(), context.as_ref())?;
        crate::runtime_snapshot::validate(context.as_ref())?;
        dispatch(&cli, &root)
    })();
    if let Err(error) = &result
        && let Some(path) = &cli.report
        && crate::report_paths::validate_explicit(&root, path).is_ok()
    {
        let mut fallback = json!({"schema":1,"tool":{"name":"cpp-infra-ci","version":env!("CARGO_PKG_VERSION"),"revision":option_env!("CPP_INFRA_BUILD_REVISION")},"bundle_id":"development","run_id":format!("error-{}",std::process::id()),"task_id":"ci","project":root,"report_root":path.parent().unwrap_or(&root),"source_revision":null,"source_digest":null,"source_excluded_paths":crate::report_paths::current()?,"config_digest":null,"status":"error","started_at":chrono::Utc::now().to_rfc3339(),"duration_ms":0,"external_versions":{},"commands":[],"diagnostics":[{"code":"CI_ERROR","severity":"error","message":error.to_string(),"project_relative_path":null}],"artifacts":[]});
        if let Some(context_path) = &cli.run_context
            && let Ok(context) = report::read(context_path)
        {
            for key in ["bundle_id", "source_digest", "config_digest", "source_revision"] {
                if let Some(value) = context.get(key) {
                    fallback[key] = value.clone();
                }
            }
        }
        save_top(&cli, &fallback)?;
    }
    result
}
fn dispatch(cli: &Cli, root: &Path) -> Result<i32, Error> {
    let (cfg, profiles) = config::load(root)?;
    if let Some(Operation::ArtifactPath {
        manifest,
        input_dir,
        task_id,
        kind,
    }) = &cli.command
    {
        let (_, manifest_bytes) = read_manifest(root, manifest)?;
        let mut plan: Plan = serde_json::from_slice(&manifest_bytes).map_err(crate::failure::configuration)?;
        crate::report_paths::merge(root, &plan.source_excluded_paths)?;
        plan.source_excluded_paths = crate::report_paths::current()?;
        planner::validate(&plan)?;
        if planner::bundle(root)? != plan.bundle_id
            || digest::project(root, false)? != plan.source_digest
            || digest::project(root, true)? != plan.config_digest
        {
            return Err(Error::Config("artifact manifest identity mismatch".into()));
        }
        let input = input_dir.canonicalize()?;
        let mut reports = BTreeMap::new();
        collect(&input, &plan, &mut reports)?;
        let node = plan
            .nodes
            .iter()
            .find(|node| node.id == *task_id)
            .ok_or_else(crate::failure::missing("unknown artifact task"))?;
        let wrapper = reports
            .get(task_id)
            .ok_or_else(crate::failure::missing("artifact task report missing"))?;
        if wrapper["status"] != "passed" {
            return Err(Error::Gate("artifact task did not pass".into()));
        }
        let wrapper_path =
            find_report(&input, &node.output)?.ok_or_else(crate::failure::missing("artifact wrapper missing"))?;
        let directory = wrapper_path
            .parent()
            .ok_or_else(crate::failure::missing("artifact wrapper parent missing"))?;
        let reference = wrapper["artifacts"]
            .as_array()
            .and_then(|artifacts| artifacts.iter().find(|artifact| artifact["kind"] == "tool-report"))
            .and_then(|artifact| artifact["path"].as_str())
            .ok_or_else(crate::failure::missing("original tool report missing"))?;
        let child = report::read(&directory.join(reference))?;
        let artifacts = child["artifacts"]
            .as_array()
            .ok_or_else(crate::failure::missing("tool artifacts missing"))?;
        let matching: Vec<_> = artifacts.iter().filter(|artifact| artifact["kind"] == *kind).collect();
        if matching.len() != 1 {
            return Err(Error::Config("expected exactly one matching tool artifact".into()));
        }
        let relative = matching[0]["path"]
            .as_str()
            .ok_or_else(crate::failure::missing("artifact path missing"))?;
        let path = crate::output_paths::no_symlinks(&directory.join(relative))?.canonicalize()?;
        if !path.starts_with(&input) {
            return Err(Error::Config("artifact escaped downloaded evidence".into()));
        }
        let result = json!({"path":path,"kind":kind,"task_id":task_id});
        let mut value = report::base(root, &input, &plan, "artifact-path", "passed");
        value["data"]["artifact"] = result.clone();
        save_top(cli, &value)?;
        println!("{}", result);
        return Ok(0);
    }

    if let Some(Operation::Export {
        input_dir,
        task_ids,
        output_dir,
        allow_missing,
    }) = &cli.command
    {
        let mut ids = BTreeSet::new();
        for id in task_ids.split(',') {
            if !ids.insert(id.into()) {
                return Err(Error::Config(format!("duplicate task id: {id}")));
            }
        }
        crate::export::tasks(input_dir, output_dir, &ids, *allow_missing)?;
        if cli.report.is_some() {
            let (_, manifest_bytes) = read_manifest(root, &input_dir.canonicalize()?.join("run-manifest.json"))?;
            let mut plan: Plan = serde_json::from_slice(&manifest_bytes).map_err(crate::failure::configuration)?;
            crate::report_paths::merge(root, &plan.source_excluded_paths)?;
            plan.source_excluded_paths = crate::report_paths::current()?;
            let mut value = report::base(root, input_dir, &plan, "export", "passed");
            value["data"]["operation"] = json!("export");
            value["data"]["exported_task_ids"] = json!(ids);
            save_top(cli, &value)?;
        }
        return Ok(0);
    }
    if let Some(Operation::Execute {
        manifest,
        task_ids,
        output_dir,
        require_existing_dependencies,
    }) = &cli.command
    {
        let (_, manifest_bytes) = read_manifest(root, manifest)?;
        let mut plan: Plan =
            serde_json::from_slice(&manifest_bytes).map_err(crate::failure::prefixed("invalid execution manifest"))?;
        crate::report_paths::merge(root, &plan.source_excluded_paths)?;
        plan.source_excluded_paths = crate::report_paths::current()?;
        planner::validate(&plan)?;
        if plan.schema != 1
            || digest::project(root, false)? != plan.source_digest
            || digest::project(root, true)? != plan.config_digest
            || planner::bundle(root)? != plan.bundle_id
        {
            return Err(Error::Config("execution manifest identity mismatch".into()));
        }
        let mut selected = BTreeSet::new();
        for id in task_ids.split(',') {
            if !selected.insert(id.into()) {
                return Err(Error::Config(format!("duplicate task id: {id}")));
            }
        }
        let report_root = if output_dir.is_absolute() {
            output_dir.clone()
        } else {
            root.join(output_dir)
        };
        let report_root = crate::output_paths::no_symlinks(&report_root)?;
        crate::output_paths::run_root(root, &report_root, &plan)?;
        fs::create_dir_all(&report_root)?;
        let mut reports = BTreeMap::new();
        collect(&report_root, &plan, &mut reports)?;
        for node in plan
            .nodes
            .iter()
            .filter(|n| selected.contains(&n.id) && n.tool != "internal")
        {
            if *require_existing_dependencies
                && node
                    .needs
                    .iter()
                    .any(|need| !selected.contains(need) && !reports.contains_key(need))
            {
                return Err(Error::Config(format!(
                    "task {} lacks required prerequisite reports",
                    node.id
                )));
            }
            if let Some(scope) = &node.execution
                && !planner::host(&scope.os, &scope.arch)
            {
                return Err(Error::Config(format!(
                    "task {} does not match execution platform",
                    node.id
                )));
            }
        }
        let (value, code) = scheduler::run_selected(root, &report_root, &plan, cli.offline, reports, Some(&selected))?;
        save_top(cli, &value)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&value).map_err(crate::failure::configuration)?
        );
        return Ok(code);
    }

    if let Some(Operation::Summarize {
        input_dir,
        manifest,
        phase,
    }) = &cli.command
    {
        let input = input_dir.canonicalize()?;
        let requested = manifest.clone().unwrap_or_else(|| input.join("run-manifest.json"));
        let (manifest_path, manifest_bytes) = read_manifest(root, &requested)?;
        let mut plan: Plan =
            serde_json::from_slice(&manifest_bytes).map_err(crate::failure::prefixed("invalid run manifest"))?;
        crate::report_paths::merge(root, &plan.source_excluded_paths)?;
        plan.source_excluded_paths = crate::report_paths::current()?;
        planner::validate(&plan)?;
        if plan.schema != 1
            || planner::bundle(root)? != plan.bundle_id
            || digest::project(root, false)? != plan.source_digest
            || digest::project(root, true)? != plan.config_digest
        {
            return Err(Error::Config(
                "summary manifest does not match current project inputs".into(),
            ));
        }
        let mut reports = BTreeMap::new();
        collect(&input, &plan, &mut reports)?;
        let mut value = report::summary(root, &input, &plan, &reports, phase);
        let manifest_sha = digest::bytes(&manifest_bytes);
        value["data"]["manifest_provenance"] =
            json!({"path":manifest_path,"sha256":manifest_sha,"external":!manifest_path.starts_with(&input)});
        if !reports.is_empty() && manifest_path.starts_with(&input) {
            let artifact = report::artifact(&input, &manifest_path, "run-manifest", true)?;
            if artifact["sha256"] != manifest_sha {
                return Err(Error::Config("manifest changed while summarizing".into()));
            }
            value["artifacts"] = json!([artifact]);
        }
        save_top(cli, &value)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&value).map_err(crate::failure::configuration)?
        );
        return Ok(if value["status"] == "passed" { 0 } else { 1 });
    }
    let (only, matrix, all) = match &cli.command {
        Some(Operation::Plan { only, .. }) | Some(Operation::Check { only }) => (only.as_deref(), None, false),
        Some(Operation::Matrix {
            command: MatrixOperation::Plan { .. },
        }) => (None, None, true),
        Some(Operation::Matrix {
            command: MatrixOperation::Run { check },
        }) => (Some("verify"), Some(check.as_str()), false),
        _ => return Err(Error::Config("a command is required; use --help".into())),
    };
    let mut plan = planner::build(root, &cfg, &profiles, only, matrix, all)?;
    let external_context = if let Some(path) = &cli.run_context {
        let context = report::read(path)?;
        validate_context(root, &plan, &context)?;
        plan.run_id = context["run_id"]
            .as_str()
            .ok_or_else(crate::failure::missing("context run_id missing"))?
            .into();
        Some(context)
    } else {
        None
    };
    match &cli.command {
        Some(Operation::Plan { output, .. }) => {
            save_plan_report(cli, root, &plan)?;
            let value = serde_json::to_value(&plan).map_err(crate::failure::configuration)?;
            if let Some(path) = output {
                report::write(path, &value)?;
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&value).map_err(crate::failure::configuration)?
            );
            Ok(0)
        }
        Some(Operation::Matrix {
            command: MatrixOperation::Plan { output },
        }) => {
            save_plan_report(cli, root, &plan)?;
            let value = crate::github_output::matrix(&cfg, &plan)?;
            if let Some(path) = output {
                report::write(path, &value)?;
                let manifest = path.parent().unwrap_or(root).join("run-manifest.json");
                report::write(
                    &manifest,
                    &serde_json::to_value(&plan).map_err(crate::failure::configuration)?,
                )?;
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&value).map_err(crate::failure::configuration)?
            );
            Ok(0)
        }
        _ => {
            let report_root = if let Some(context) = external_context {
                PathBuf::from(
                    context["report_root"]
                        .as_str()
                        .ok_or_else(crate::failure::missing("context report_root missing"))?,
                )
            } else {
                root.join("build/infra-runs").join(&plan.run_id)
            };
            if !report_root.is_absolute() {
                return Err(Error::Config("report_root must be absolute".into()));
            }
            let (value, code) = scheduler::run(root, &report_root, &plan, cli.offline)?;
            save_top(cli, &value)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&value).map_err(crate::failure::configuration)?
            );
            Ok(code)
        }
    }
}
fn validate_context(root: &Path, plan: &Plan, context: &Value) -> Result<(), Error> {
    let project = context["project"]
        .as_str()
        .ok_or_else(crate::failure::missing("context project missing"))?;
    if context["schema"] != 1
        || Path::new(project).canonicalize()? != root
        || context["bundle_id"] != plan.bundle_id
        || context["source_digest"] != plan.source_digest
        || context["config_digest"] != plan.config_digest
    {
        return Err(Error::Config("stale or mismatched run context".into()));
    }
    let run_id = context["run_id"]
        .as_str()
        .ok_or_else(crate::failure::missing("context run_id missing"))?;
    config::identifier(run_id)?;
    let report_root = Path::new(
        context["report_root"]
            .as_str()
            .ok_or_else(crate::failure::missing("context report_root missing"))?,
    );
    let expected = root.join("build/infra-runs").join(run_id);
    if report_root != expected {
        return Err(Error::Config(
            "context report_root is outside the canonical run directory".into(),
        ));
    }
    for ancestor in report_root.ancestors() {
        if ancestor.exists() {
            if !ancestor.canonicalize()?.starts_with(root) {
                return Err(Error::Config(
                    "context report_root traverses an escaping symlink".into(),
                ));
            }
            break;
        }
    }
    if context["task_id"].as_str().is_none() {
        return Err(Error::Config("context task_id missing".into()));
    }
    Ok(())
}
fn collect(dir: &Path, plan: &Plan, reports: &mut BTreeMap<String, Value>) -> Result<(), Error> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            collect(&path, plan, reports)?;
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(node) = plan.nodes.iter().find(|n| n.output == name) else {
            continue;
        };
        if node.id == "final-summary" {
            continue;
        }
        let value = report::read(&path)?;
        report::verify(&value, plan, &node.id)?;
        if value["data"]["matrix_digest"] != plan.matrix_digest {
            return Err(Error::Config(format!("different matrix definition for {}", node.id)));
        }
        if reports.contains_key(&node.id) {
            return Err(Error::Config(format!("duplicate task report: {}", node.id)));
        }
        report::verify_wrapper(
            &value,
            plan,
            node,
            path.parent()
                .ok_or_else(crate::failure::missing("report has no parent"))?,
        )?;
        if reports.insert(node.id.clone(), value).is_some() {
            return Err(Error::Config(format!("duplicate task report: {}", node.id)));
        }
    }
    Ok(())
}

fn save_top(cli: &Cli, value: &Value) -> Result<(), Error> {
    if let Some(path) = &cli.report {
        let mut top = value.clone();
        if let Some(context_path) = &cli.run_context {
            let context = report::read(context_path)?;
            top["task_id"] = context["task_id"].clone();
            top["run_id"] = context["run_id"].clone();
            top["data"]["result_report_root"] = value["report_root"].clone();
            top["report_root"] = context["report_root"].clone();
            top["artifacts"] = json!([]);
        }
        report::write(path, &top)?;
    }
    Ok(())
}
fn save_plan_report(cli: &Cli, root: &Path, plan: &Plan) -> Result<(), Error> {
    if let Some(path) = &cli.report {
        let report_root = path.parent().unwrap_or(root);
        let mut value = report::base(root, report_root, plan, "plan", "passed");
        value["data"]["operation"] = json!("plan");
        value["data"]["executed"] = json!(false);
        save_top(cli, &value)?;
    }
    Ok(())
}

fn find_report(directory: &Path, name: &str) -> Result<Option<PathBuf>, Error> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            if let Some(path) = find_report(&entry.path(), name)? {
                return Ok(Some(path));
            }
        } else if entry.file_name() == name {
            return Ok(Some(entry.path()));
        }
    }
    Ok(None)
}

/// Captures exact manifest bytes once and resolves CLI relative paths from the
/// project.
fn read_manifest(root: &Path, path: &Path) -> Result<(PathBuf, Vec<u8>), Error> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let path = path.canonicalize()?;
    let bytes = fs::read(&path)?;
    Ok((path, bytes))
}
