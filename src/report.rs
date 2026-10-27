// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Atomic protocol report production and verification.
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value;
use serde_json::json;

use crate::digest;
use crate::error::Error;
use crate::node::Node;
use crate::plan::Plan;

pub(crate) fn base(root: &Path, report_root: &Path, plan: &Plan, task: &str, status: &str) -> Value {
    json!({"schema":1,"tool":{"name":"cpp-infra-ci","version":env!("CARGO_PKG_VERSION"),"revision":option_env!("CPP_INFRA_BUILD_REVISION")},"bundle_id":plan.bundle_id,"run_id":plan.run_id,"task_id":task,"project":root,"report_root":report_root,"source_revision":plan.source_revision,"source_digest":plan.source_digest,"source_excluded_paths":plan.source_excluded_paths,"config_digest":plan.config_digest,"status":status,"started_at":chrono::Utc::now().to_rfc3339(),"duration_ms":0,"external_versions":{},"commands":[],"diagnostics":[],"artifacts":[],"data":{"matrix_digest":plan.matrix_digest}})
}
pub(crate) fn write(path: &Path, value: &Value) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(
        &temp,
        serde_json::to_vec_pretty(value).map_err(crate::failure::configuration)?,
    )?;
    fs::rename(temp, path)?;
    Ok(())
}
pub(crate) fn read(path: &Path) -> Result<Value, Error> {
    serde_json::from_slice(&fs::read(path)?).map_err(crate::failure::file(path))
}
pub(crate) fn verify(value: &Value, plan: &Plan, task: &str) -> Result<(), Error> {
    if value["schema"] != 1
        || value["task_id"] != task
        || value["run_id"] != plan.run_id
        || value["bundle_id"] != plan.bundle_id
        || value["source_digest"] != plan.source_digest
        || value["config_digest"] != plan.config_digest
        || value["source_revision"] != json!(plan.source_revision)
    {
        return Err(Error::Config(format!("report identity mismatch: {task}")));
    }
    for field in [
        "tool",
        "project",
        "report_root",
        "started_at",
        "duration_ms",
        "external_versions",
        "commands",
        "diagnostics",
        "artifacts",
    ] {
        if value.get(field).is_none() {
            return Err(Error::Config(format!("report {task} missing {field}")));
        }
    }
    if !["passed", "failed", "error", "cancelled", "skipped", "blocked"]
        .contains(&value["status"].as_str().unwrap_or(""))
    {
        return Err(Error::Config(format!("invalid report status: {task}")));
    }
    if value["tool"]["name"].as_str().is_none()
        || value["tool"]["version"].as_str().is_none()
        || value["tool"].get("revision").is_none()
    {
        return Err(Error::Config(format!("invalid report tool identity: {task}")));
    }
    Ok(())
}
pub(crate) fn artifact(root: &Path, path: &Path, kind: &str, required: bool) -> Result<Value, Error> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| Error::Config(format!("artifact outside report root: {}", path.display())))?;
    Ok(
        json!({"kind":kind,"path":relative.to_string_lossy().replace('\\',"/"),"sha256":digest::file(path)?,"required":required}),
    )
}
pub(crate) fn validate_artifacts(value: &Value) -> Result<(), Error> {
    let report_root = Path::new(
        value["report_root"]
            .as_str()
            .ok_or_else(crate::failure::missing("report_root must be a path"))?,
    )
    .canonicalize()?;
    validate_artifacts_at(value, &report_root)
}
pub(crate) fn validate_artifacts_at(value: &Value, root: &Path) -> Result<(), Error> {
    let report_root = root.canonicalize()?;
    for item in value["artifacts"]
        .as_array()
        .ok_or_else(crate::failure::missing("artifacts must be an array"))?
    {
        let path = Path::new(
            item["path"]
                .as_str()
                .ok_or_else(crate::failure::missing("artifact path must be a string"))?,
        );
        if path.is_absolute() || path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err(Error::Config("artifact path escapes report root".into()));
        }
        let path = report_root.join(path).canonicalize()?;
        if !path.starts_with(&report_root) || digest::artifact(&path)? != item["sha256"] {
            return Err(Error::Config("artifact path/digest mismatch".into()));
        }
    }
    Ok(())
}
pub(crate) fn summary(
    root: &Path,
    report_root: &Path,
    plan: &Plan,
    reports: &BTreeMap<String, Value>,
    phase: &str,
) -> Value {
    let expected: Vec<_> = plan
        .nodes
        .iter()
        .filter(|n| {
            n.id != "final-summary"
                && (phase == "final" || (n.id != "pages" && n.id != "quality-summary" && n.id != "doctor-pages"))
        })
        .collect();
    let mut complete = true;
    let mut passed = true;
    let mut tasks = Vec::new();
    for node in expected {
        let report = reports.get(&node.id);
        let status = report.and_then(|v| v["status"].as_str()).unwrap_or("missing");
        let valid = report.is_some_and(|v| {
            verify_wrapper(v, plan, node, report_root).is_ok() && v["data"]["matrix_digest"] == plan.matrix_digest
        });
        if node.required && (!valid || status == "missing" || status == "blocked") {
            complete = false;
        }
        if node.required && (!valid || status != "passed") {
            passed = false;
        }
        tasks.push(json!({"task_id":node.id,"status":status,"required":node.required,"report":node.output}));
    }
    let mut value = base(
        root,
        report_root,
        plan,
        if phase == "quality" {
            "quality-summary"
        } else {
            "final-summary"
        },
        if passed && complete && !(phase == "final" && plan.require_complete_matrix && plan.incomplete_remote_coverage)
        {
            "passed"
        } else {
            "failed"
        },
    );
    value["data"] = json!({"phase":phase,"complete":complete,"tasks":tasks,"expected_task_ids":tasks.iter().filter_map(|t|t["task_id"].as_str()).collect::<Vec<_>>(),"matrix_digest":plan.matrix_digest,"incomplete_remote_coverage":plan.incomplete_remote_coverage,"remote_only":plan.remote_only});
    value
}

pub(crate) fn validate_evidence_at(value: &Value, root: &Path) -> Result<(), Error> {
    validate_evidence(value, root, 0)
}
fn validate_evidence(value: &Value, root: &Path, depth: usize) -> Result<(), Error> {
    if depth > 32 {
        return Err(Error::Config("report evidence nesting exceeds 32".into()));
    }
    let project = crate::report_paths::project_root();
    if let Some(project) = project
        && project.is_dir()
    {
        let exclusions = crate::report_paths::parse(value.get("source_excluded_paths"))?;
        crate::report_paths::validate_list(project, &exclusions)?;
    }
    validate_artifacts_at(value, root)?;
    for artifact in value["artifacts"]
        .as_array()
        .ok_or_else(crate::failure::missing("report artifacts missing"))?
    {
        if artifact["kind"] == "tool-report" {
            let path = artifact["path"]
                .as_str()
                .ok_or_else(crate::failure::missing("child report path missing"))?;
            let child = read(&root.join(path))?;
            validate_evidence(&child, root, depth + 1)?;
        }
    }
    Ok(())
}

pub(crate) fn verify_wrapper(value: &Value, plan: &Plan, node: &Node, root: &Path) -> Result<(), Error> {
    verify(value, plan, &node.id)?;
    if value["tool"] != json!(plan.ci_tool) {
        return Err(Error::Config(format!("CI report producer mismatch: {}", node.id)));
    }
    if value["data"]["matrix_digest"] != plan.matrix_digest {
        return Err(Error::Config(format!("report matrix mismatch: {}", node.id)));
    }
    validate_evidence_at(value, root)
        .map_err(|error| Error::Config(format!("invalid report evidence for {}: {error}", node.id)))?;
    if node.tool == "internal" {
        return Ok(());
    }
    let passed = value["status"] == "passed";
    if passed
        && !value["commands"]
            .as_array()
            .is_some_and(|commands| commands.iter().any(|command| command["exit_code"] == 0))
    {
        return Err(Error::Config(format!(
            "passed task has no successful recorded command: {}",
            node.id
        )));
    }
    if node.tool.starts_with("project:") {
        return Ok(());
    }
    let artifacts = value["artifacts"]
        .as_array()
        .ok_or_else(crate::failure::missing("wrapper artifacts missing"))?;
    let children: Vec<_> = artifacts
        .iter()
        .filter(|artifact| artifact["kind"] == "tool-report")
        .collect();
    if children.is_empty() && !passed {
        return Ok(());
    }
    if children.len() != 1 || children[0]["required"] != true {
        return Err(Error::Config(format!(
            "task {} must retain one required original tool report",
            node.id
        )));
    }
    let path = children[0]["path"]
        .as_str()
        .ok_or_else(crate::failure::missing("tool report path missing"))?;
    let child = read(&root.join(path))?;
    verify(&child, plan, &node.id)?;
    if child["tool"]["name"] != node.tool
        || child["tool"]["revision"] != json!(node.revision)
        || node
            .version
            .as_ref()
            .is_some_and(|version| child["tool"]["version"] != *version)
    {
        return Err(Error::Config(format!(
            "original child tool/version/revision mismatch: {}",
            node.id
        )));
    }
    if passed && child["status"] != "passed" {
        return Err(Error::Config(format!(
            "passed wrapper contradicts original child result: {}",
            node.id
        )));
    }
    Ok(())
}
