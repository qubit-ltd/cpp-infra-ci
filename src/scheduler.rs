// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Bounded DAG execution with real child-report evidence.
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::fs::File;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use serde_json::Value;
use serde_json::json;

use crate::digest;
use crate::error::Error;
use crate::node::Node;
use crate::plan::Plan;
use crate::report;

pub(crate) fn run(root: &Path, report_root: &Path, plan: &Plan, offline: bool) -> Result<(Value, i32), Error> {
    run_selected(root, report_root, plan, offline, BTreeMap::new(), None)
}
pub(crate) fn run_selected(
    root: &Path,
    report_root: &Path,
    plan: &Plan,
    offline: bool,
    mut reports: BTreeMap<String, Value>,
    selected: Option<&BTreeSet<String>>,
) -> Result<(Value, i32), Error> {
    crate::output_paths::run_root(root, report_root, plan)?;
    fs::create_dir_all(report_root)?;
    let runtime_snapshot = crate::runtime_snapshot::for_report_root(report_root)?;
    report::write(
        &report_root.join("run-manifest.json"),
        &serde_json::to_value(plan).map_err(crate::failure::configuration)?,
    )?;
    let mut remaining: BTreeSet<String> = if let Some(selected) = selected {
        let mut pending = selected.clone();
        let mut stack: Vec<String> = selected.iter().cloned().collect();
        while let Some(id) = stack.pop() {
            let node = plan
                .nodes
                .iter()
                .find(|n| n.id == id)
                .ok_or_else(|| Error::Config(format!("unknown task id: {id}")))?;
            if node.tool == "internal" {
                continue;
            }
            for need in &node.needs {
                if !reports.contains_key(need) && pending.insert(need.clone()) {
                    stack.push(need.clone());
                }
            }
        }
        for id in selected {
            reports.remove(id);
        }
        pending
    } else {
        plan.nodes.iter().map(|n| n.id.clone()).collect()
    };
    let mut exit_code = 0;
    let mut first_failure: Option<String> = None;
    while !remaining.is_empty() {
        let mut wave = Vec::new();
        let mut resources = BTreeSet::new();
        for node in &plan.nodes {
            if !remaining.contains(&node.id)
                || !(node.needs.iter().all(|id| reports.contains_key(id))
                    || node.tool == "internal" && node.needs.iter().all(|id| !remaining.contains(id)))
            {
                continue;
            }
            if node.tool == "internal" {
                let phase = if node.id == "quality-summary" {
                    "quality"
                } else {
                    "final"
                };
                let mut value = report::summary(root, report_root, plan, &reports, phase);
                value["data"]["first_failure"] = json!(first_failure);
                report::write(&report_root.join(&node.output), &value)?;
                reports.insert(node.id.clone(), value);
                remaining.remove(&node.id);
                continue;
            }
            let prerequisite_failed = node
                .needs
                .iter()
                .any(|need| reports.get(need).is_some_and(|v| v["status"] != "passed"));
            let pages = node.id == "pages";
            let pages_doctor_failed = pages && reports.get("doctor-pages").is_some_and(|v| v["status"] != "passed");
            if crate::cancellation::requested() {
                exit_code = 130;
                let mut value = report::base(root, report_root, plan, &node.id, "cancelled");
                value["diagnostics"] = json!([{"code":"CI_CANCELLED","severity":"error","message":"run cancelled","project_relative_path":null}]);
                report::write(&report_root.join(&node.output), &value)?;
                reports.insert(node.id.clone(), value);
                remaining.remove(&node.id);
                continue;
            }
            if pages_doctor_failed
                || (!pages && prerequisite_failed)
                || (!pages && plan.fail_fast && first_failure.is_some())
            {
                let mut value = report::base(root, report_root, plan, &node.id, "blocked");
                value["diagnostics"] = json!([{"code":"CI_BLOCKED","severity":"error","message":"prerequisite failed or local fail-fast stopped scheduling","project_relative_path":null}]);
                report::write(&report_root.join(&node.output), &value)?;
                reports.insert(node.id.clone(), value);
                remaining.remove(&node.id);
                continue;
            }
            let locks = resource_locks(node, plan, report_root)?;
            if wave.len() < plan.task_jobs && !locks.iter().any(|r| resources.contains(r)) {
                resources.extend(locks);
                wave.push(node.clone());
            }
        }
        if wave.is_empty() {
            if remaining.is_empty() {
                break;
            }
            return Err(Error::Config("scheduler made no progress".into()));
        }
        let outcomes = thread::scope(|scope| {
            let mut handles = Vec::new();
            for node in &wave {
                let reports_ref = &reports;
                handles.push((
                    node,
                    scope.spawn({
                        let snapshot = runtime_snapshot
                            .as_ref()
                            .map(|(path, digest)| (path.as_path(), digest.as_str()));
                        move || execute(root, report_root, plan, node, offline, reports_ref, snapshot)
                    }),
                ));
            }
            handles.into_iter().map(|(n, h)| (n, h.join())).collect::<Vec<_>>()
        });
        for (node, result) in outcomes {
            let (value, code) = match result {
                Ok(Ok(out)) => out,
                Ok(Err(error)) => {
                    let mut value = report::base(root, report_root, plan, &node.id, "error");
                    value["diagnostics"] = json!([{"code":"CI_INFRASTRUCTURE","severity":"error","message":error.to_string(),"project_relative_path":null}]);
                    (value, error.exit_code())
                }
                Err(_) => return Err(Error::Config("task worker panicked".into())),
            };
            if code != 0 && node.required {
                if first_failure.is_none() {
                    first_failure = Some(node.id.clone());
                }
                exit_code = priority(exit_code, code);
            }
            report::write(&report_root.join(&node.output), &value)?;
            reports.insert(node.id.clone(), value);
            remaining.remove(&node.id);
        }
    }
    let final_report = if selected.is_none_or(|ids| ids.contains("final-summary")) {
        reports
            .remove("final-summary")
            .ok_or_else(crate::failure::missing("missing final summary"))?
    } else if selected.is_some_and(|ids| ids.contains("quality-summary")) {
        reports
            .remove("quality-summary")
            .ok_or_else(crate::failure::missing("missing quality summary"))?
    } else {
        let selected = selected.ok_or_else(crate::failure::missing("missing selection"))?;
        let passed = selected.iter().all(|id| {
            plan.nodes.iter().any(|n| n.id == *id && !n.required)
                || reports.get(id).is_some_and(|v| v["status"] == "passed")
        });
        let mut value = report::base(
            root,
            report_root,
            plan,
            "execution",
            if passed { "passed" } else { "failed" },
        );
        value["data"]["executed_task_ids"] = json!(selected);
        value
    };
    if final_report["status"] != "passed" {
        exit_code = priority(exit_code, 1);
    }
    Ok((final_report, exit_code))
}
fn priority(previous: i32, next: i32) -> i32 {
    if previous == 130 || next == 130 {
        130
    } else {
        previous.max(next)
    }
}
fn executable(tool: &str) -> Result<PathBuf, Error> {
    if Path::new(tool).is_absolute() {
        return Ok(tool.into());
    }
    if let Some(dir) = std::env::var_os("CPP_INFRA_BIN_DIR") {
        let path = PathBuf::from(dir).join(if cfg!(windows) {
            format!("{tool}.exe")
        } else {
            tool.into()
        });
        if !path.is_file() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("locked binary missing: {}", path.display()),
            )));
        }
        return Ok(path);
    }
    Ok(tool.into())
}
fn execute(
    root: &Path,
    report_root: &Path,
    plan: &Plan,
    node: &Node,
    offline: bool,
    reports: &BTreeMap<String, Value>,
    runtime_snapshot: Option<(&Path, &str)>,
) -> Result<(Value, i32), Error> {
    if digest::project(root, false)? != plan.source_digest || digest::project(root, true)? != plan.config_digest {
        return Err(Error::Config("source/configuration changed since planning".into()));
    }
    let started = Instant::now();
    let mut value = report::base(root, report_root, plan, &node.id, "error");
    let context_path = report_root.join(format!("{}-context.json", node.id));
    let child_path = report_root.join(format!("{}-tool.json", node.id));
    let mut context = json!({"schema":1,"run_id":plan.run_id,"task_id":node.id,"bundle_id":plan.bundle_id,"project":root,"profile":node.profile.as_deref().unwrap_or(&plan.primary_profile),"source_digest":plan.source_digest,"source_excluded_paths":plan.source_excluded_paths,"config_digest":plan.config_digest,"report_root":report_root});
    if let Some((path, digest)) = runtime_snapshot {
        context["runtime_snapshot"] = json!(path);
        context["runtime_snapshot_digest"] = json!(digest);
    }
    if node.args.first().is_none_or(|operation| operation != "prepare")
        && let Some(path) = build_context_path(node, plan, report_root)
        && path.is_file()
    {
        let build_context = report::read(&path)?;
        if build_context["project"].as_str() == root.to_str()
            && build_context["binary_dir"]
                .as_str()
                .is_some_and(|p| Path::new(p).is_dir())
        {
            context["build_context"] = json!(path);
        }
    }
    if node.id == "pages" {
        let quality = report_root.join("quality-summary.json");
        let mut artifacts =
            vec![json!({"kind":"quality-summary","path":quality,"sha256":digest::file(&quality)?,"required":true})];
        if reports.get("coverage").is_some_and(|v| v["status"] == "passed") {
            let path = report_root.join("coverage-tool.json");
            artifacts
                .push(json!({"kind":"coverage-summary","path":path,"sha256":digest::file(&path)?,"required":false}));
        }
        context["artifacts"] = json!(artifacts);
    }
    report::write(&context_path, &context)?;
    let is_hook = node.tool.starts_with("project:");
    let mut args = Vec::new();
    if !is_hook {
        args.extend([
            "--project".into(),
            root.to_string_lossy().into_owned(),
            "--report".into(),
            child_path.to_string_lossy().into_owned(),
            "--run-context".into(),
            context_path.to_string_lossy().into_owned(),
        ]);
        if offline {
            args.push("--offline".into());
        }
    }
    args.extend(
        node.args
            .iter()
            .map(|s| s.replace("@REPORT_ROOT@", &report_root.to_string_lossy())),
    );
    let log_path = report_root.join(format!("{}.log", node.id));
    let log = File::create(&log_path)?;
    let program = if let Some(path) = node.tool.strip_prefix("project:") {
        crate::config::contained(root, path)?
    } else {
        executable(&node.tool)?
    };
    let mut command = Command::new(&program);
    command
        .args(&args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    let environment = if let Some((path, _)) = runtime_snapshot {
        command.env("CPP_INFRA_RUNTIME_SNAPSHOT", path);
        json!({"CPP_INFRA_RUNTIME_SNAPSHOT": path})
    } else {
        json!({})
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x00000200);
    }
    let mut child = command.spawn()?;
    let mut timed_out = false;
    let mut cancelled = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if crate::cancellation::requested() {
            cancelled = true;
            terminate(&mut child);
            break child.wait()?;
        }
        if started.elapsed() >= Duration::from_secs(node.timeout_seconds) {
            timed_out = true;
            terminate(&mut child);
            break child.wait()?;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let code = status.code().unwrap_or(if timed_out { 1 } else { 130 });
    value["commands"] = json!([{"program":program,"args":args,"cwd":root,"env":environment,"exit_code":code,"duration_ms":started.elapsed().as_millis() as u64,"log":format!("{}.log",node.id)}]);
    value["duration_ms"] = json!(started.elapsed().as_millis() as u64);
    if cancelled {
        value["status"] = json!("cancelled");
        value["diagnostics"] = json!([{"code":"CI_CANCELLED","severity":"error","message":"child process tree cancelled","project_relative_path":null}]);
        return Ok((value, 130));
    }
    if timed_out {
        value["status"] = json!("failed");
        value["diagnostics"] = json!([{"code":"CI_TIMEOUT","severity":"error","message":"task exceeded timeout; child process tree terminated","project_relative_path":null}]);
        return Ok((value, 1));
    }
    if digest::project(root, false)? != plan.source_digest || digest::project(root, true)? != plan.config_digest {
        value["status"] = json!("error");
        value["diagnostics"] = json!([{"code":"CI_INPUT_CHANGED","severity":"error","message":"project inputs changed while a task was running","project_relative_path":null}]);
        return Ok((value, 2));
    }
    if is_hook {
        value["status"] = json!(if code == 0 { "passed" } else { "failed" });
        return Ok((value, if code == 0 { 0 } else { 1 }));
    }
    if !child_path.is_file() {
        let mapped = match code {
            0 => 2,
            1 | 2 | 3 | 130 => code,
            _ => 3,
        };
        value["status"] = json!(if mapped == 1 {
            "failed"
        } else if mapped == 130 {
            "cancelled"
        } else {
            "error"
        });
        value["diagnostics"] = json!([{"code":"CI_CHILD_REPORT_MISSING","severity":"error","message":format!("child exited {code} without a public report; see {}.log",node.id),"project_relative_path":null}]);
        value["artifacts"] = json!([report::artifact(report_root, &log_path, "log", true)?]);
        return Ok((value, mapped));
    }
    let child_report = report::read(&child_path)?;
    report::verify(&child_report, plan, &node.id)?;
    if child_report["tool"]["name"] != node.tool {
        return Err(Error::Config(format!("unexpected child tool for {}", node.id)));
    }
    if node
        .revision
        .as_ref()
        .is_some_and(|revision| child_report["tool"]["revision"] != *revision)
    {
        return Err(Error::Config(format!("child revision mismatch: {}", node.id)));
    }
    report::validate_artifacts(&child_report)?;
    value["data"]["child_tool"] = json!(node.tool);
    value["data"]["child_report"] = json!(format!("{}-tool.json", node.id));
    value["artifacts"] = json!([report::artifact(report_root, &child_path, "tool-report", true)?]);
    value["status"] = child_report["status"].clone();
    value["diagnostics"] = child_report["diagnostics"].clone();
    value["external_versions"] = child_report["external_versions"].clone();
    let mapped = match code {
        0 => {
            if child_report["status"] == "passed" {
                0
            } else {
                1
            }
        }
        1 | 2 | 3 | 130 => code,
        _ => 3,
    };
    if mapped != 0 && value["status"] == "passed" {
        value["status"] = json!(if mapped == 1 {
            "failed"
        } else if mapped == 130 {
            "cancelled"
        } else {
            "error"
        });
    }
    Ok((value, mapped))
}
fn terminate(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32; // The child starts in its own process group.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

fn build_context_path(node: &Node, plan: &Plan, root: &Path) -> Option<PathBuf> {
    let prepare = if node.args.first().is_some_and(|argument| argument == "prepare") {
        Some(node)
    } else {
        node.needs.iter().find_map(|id| {
            plan.nodes.iter().find(|candidate| {
                candidate.id == *id
                    && candidate.tool == "cpp-infra-verify"
                    && candidate.args.first().is_some_and(|argument| argument == "prepare")
            })
        })
    }?;
    Some(root.join(format!("{}-build-context.json", prepare.id)))
}
fn resource_locks(node: &Node, plan: &Plan, report_root: &Path) -> Result<Vec<String>, Error> {
    let mut locks = node.resource_locks.clone();
    if let Some(path) = build_context_path(node, plan, report_root)
        && path.exists()
    {
        let context = report::read(&path)?;
        if let Some(binary) = context["binary_dir"].as_str() {
            locks.push(format!("binary-dir:{}", Path::new(binary).canonicalize()?.display()));
        }
    }
    Ok(locks)
}
