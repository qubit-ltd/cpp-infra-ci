// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Deterministic dependency expansion and cycle detection.
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use serde_json::Value;

use crate::config::Config;
use crate::config::Profile;
use crate::config::require_profile;
use crate::config::task_profile;
use crate::digest;
use crate::error::Error;
use crate::execution_scope::ExecutionScope;
use crate::node::Node;
use crate::plan::Plan;
use crate::tool_identity::ToolIdentity;

static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);
pub(crate) fn host(entry_os: &str, entry_arch: &str) -> bool {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else {
        std::env::consts::OS
    };
    entry_os == os
        && (entry_arch == std::env::consts::ARCH || (entry_arch == "arm64" && std::env::consts::ARCH == "aarch64"))
}
pub(crate) fn build(
    root: &Path,
    cfg: &Config,
    profiles: &BTreeMap<String, Profile>,
    only: Option<&str>,
    matrix: Option<&str>,
    all: bool,
) -> Result<Plan, Error> {
    let mut selected: Vec<String> = if let Some(only) = only {
        let mut seen = BTreeSet::new();
        let mut list = Vec::new();
        for task in only.split(',') {
            if !cfg.tasks.iter().any(|t| t == task) || !seen.insert(task) {
                return Err(Error::Config(format!("unknown or duplicate --only task: {task}")));
            }
        }
        for task in &cfg.tasks {
            if seen.contains(task.as_str()) {
                list.push(task.clone());
            }
        }
        list
    } else {
        cfg.tasks.clone()
    };
    if selected.iter().any(|task| task == "pages") {
        selected = cfg.tasks.clone();
    }
    let mut plan = Plan {
        schema: 1,
        ci_tool: ToolIdentity::current(),
        bundle_id: bundle(root)?,
        run_id: format!(
            "{}-{}-{}",
            chrono::Utc::now().format("%Y%m%dT%H%M%S%fZ"),
            std::process::id(),
            RUN_COUNTER.fetch_add(1, Ordering::Relaxed)
        ),
        source_digest: digest::project(root, false)?,
        source_excluded_paths: crate::report_paths::current()?,
        config_digest: digest::project(root, true)?,
        source_revision: revision(root)?,
        matrix_digest: digest::bytes(&serde_json::to_vec(&cfg.matrix).map_err(crate::failure::configuration)?),
        nodes: Vec::new(),
        remote_only: Vec::new(),
        incomplete_remote_coverage: false,
        primary_profile: cfg.primary_profile.clone(),
        require_complete_matrix: cfg.require_complete_matrix,
        task_jobs: cfg.task_jobs,
        fail_fast: if all { false } else { cfg.local_fail_fast },
    };
    if let Some(id) = matrix {
        let entry = cfg
            .matrix
            .iter()
            .find(|m| m.id == id)
            .ok_or_else(|| Error::Config(format!("unknown matrix check: {id}")))?;
        if !host(&entry.os, &entry.arch) {
            return Err(Error::Config(format!("matrix {id} does not match host")));
        }
    }
    for task in &selected {
        if task == "verify" {
            plan_verify(&mut plan, cfg, matrix, all)?;
            continue;
        }
        if task == "package" {
            plan_package(&mut plan, cfg, all);
            continue;
        }
        if task == "pages" {
            continue;
        }
        let doctor_profile = if task == "analyze" || task == "coverage" {
            task_profile(root, task, profiles)?
        } else {
            cfg.primary_profile.clone()
        };
        let doctor_id = format!("doctor-{task}");
        add(
            &mut plan,
            &doctor_id,
            "tools",
            vec![
                "doctor".into(),
                "--task".into(),
                task.clone(),
                "--profile".into(),
                doctor_profile.clone(),
            ],
            if task == "analyze" || task == "coverage" || task == "dependency" || task == "package" {
                Some(doctor_profile.clone())
            } else {
                None
            },
            Vec::new(),
            cfg.timeout_seconds,
        );
        match task.as_str() {
            "style" => {
                add(
                    &mut plan,
                    "style",
                    "style",
                    vec!["check".into()],
                    None,
                    vec![doctor_id],
                    cfg.timeout_seconds,
                );
            }
            "dependency" => {
                let context = profiles
                    .get(&cfg.primary_profile)
                    .and_then(|p| p.conan_context.as_ref())
                    .ok_or_else(crate::failure::missing(
                        "dependency requires primary profile conan_context",
                    ))?;
                add(
                    &mut plan,
                    "dependency",
                    "dependency",
                    vec!["check".into(), "--context".into(), context.clone()],
                    Some(cfg.primary_profile.clone()),
                    vec![doctor_id],
                    cfg.timeout_seconds,
                );
            }
            "analyze" | "coverage" => {
                let profile = task_profile(root, task, profiles)?;
                require_profile(profiles, &profile)?;
                let prep = format!("prepare-{profile}");
                if !plan.nodes.iter().any(|n| n.id == prep) {
                    add(
                        &mut plan,
                        &prep,
                        "verify",
                        vec![
                            "prepare".into(),
                            "--profile".into(),
                            profile.clone(),
                            "--output".into(),
                            format!("@REPORT_ROOT@/{prep}-build-context.json"),
                        ],
                        Some(profile.clone()),
                        vec![doctor_id.clone()],
                        cfg.timeout_seconds,
                    );
                }
                add(
                    &mut plan,
                    task,
                    task,
                    if task == "analyze" {
                        vec!["check".into(), "--profile".into(), profile.clone()]
                    } else {
                        vec!["collect".into()]
                    },
                    Some(profile),
                    vec![doctor_id, prep],
                    cfg.timeout_seconds,
                );
            }
            "pages" => {}
            _ => return Err(Error::Config(format!("unknown task {task}"))),
        }
    }
    if only.is_none() || selected.iter().any(|task| task == "pages") {
        for hook in &cfg.hooks {
            let mut node = Node {
                id: hook.id.clone(),
                tool: format!("project:{}", hook.executable),
                args: hook.args.clone(),
                profile: None,
                execution: None,
                version: None,
                needs: hook.needs.clone(),
                resource_locks: vec!["project-hook".into()],
                timeout_seconds: hook.timeout_seconds,
                required: hook.required,
                output: format!("{}.json", hook.id),
                revision: None,
            };
            if node.needs.is_empty() {
                node.needs = plan
                    .nodes
                    .iter()
                    .filter(|n| n.tool != "internal")
                    .map(|n| n.id.clone())
                    .collect();
            }
            plan.nodes.push(node);
        }
    }
    let quality: Vec<String> = plan.nodes.iter().map(|n| n.id.clone()).collect();
    add(
        &mut plan,
        "quality-summary",
        "internal",
        Vec::new(),
        None,
        quality,
        cfg.timeout_seconds,
    );
    if selected.iter().any(|t| t == "pages") {
        add(
            &mut plan,
            "doctor-pages",
            "tools",
            vec![
                "doctor".into(),
                "--task".into(),
                "pages".into(),
                "--profile".into(),
                cfg.primary_profile.clone(),
            ],
            None,
            vec!["quality-summary".into()],
            cfg.timeout_seconds,
        );
        add(
            &mut plan,
            "pages",
            "pages",
            vec!["build".into()],
            None,
            vec!["quality-summary".into(), "doctor-pages".into()],
            cfg.timeout_seconds,
        );
    }
    let final_needs = plan
        .nodes
        .iter()
        .filter(|n| n.id == "quality-summary" || n.id == "pages")
        .map(|n| n.id.clone())
        .collect();
    add(
        &mut plan,
        "final-summary",
        "internal",
        Vec::new(),
        None,
        final_needs,
        cfg.timeout_seconds,
    );
    for node in &mut plan.nodes {
        if let Some(profile) = node.profile.as_ref().and_then(|p| profiles.get(p)) {
            node.resource_locks = vec![format!("configure:{}", profile.configure)];
            if let Some(context) = &profile.conan_context {
                node.resource_locks.push(format!("dependency:{context}"));
            }
        }
    }
    plan.incomplete_remote_coverage = !plan.remote_only.is_empty();
    pin_revisions(root, &mut plan)?;
    validate(&plan)?;
    Ok(plan)
}
fn add(
    plan: &mut Plan,
    id: &str,
    tool: &str,
    args: Vec<String>,
    profile: Option<String>,
    needs: Vec<String>,
    timeout_seconds: u64,
) {
    let resource_locks = profile.iter().map(|p| format!("profile:{p}")).collect();
    plan.nodes.push(Node {
        id: id.into(),
        tool: if tool == "internal" {
            tool.into()
        } else {
            format!("cpp-infra-{tool}")
        },
        args,
        profile,
        execution: None,
        version: None,
        needs,
        resource_locks,
        timeout_seconds,
        required: true,
        output: format!("{id}.json"),
        revision: None,
    });
}
fn add_verify(plan: &mut Plan, id: &str, profile: &str, suite: &str, mut needs: Vec<String>, timeout: u64) {
    let prepare = format!("prepare-{profile}");
    if !plan.nodes.iter().any(|n| n.id == prepare) {
        add(
            plan,
            &prepare,
            "verify",
            vec![
                "prepare".into(),
                "--profile".into(),
                profile.into(),
                "--output".into(),
                format!("@REPORT_ROOT@/{prepare}-build-context.json"),
            ],
            Some(profile.into()),
            needs.clone(),
            timeout,
        );
    }
    needs.push(prepare);
    add(
        plan,
        id,
        "verify",
        vec![
            "run".into(),
            "--profile".into(),
            profile.into(),
            "--suite".into(),
            suite.into(),
        ],
        Some(profile.into()),
        needs,
        timeout,
    );
}
pub(crate) fn validate(plan: &Plan) -> Result<(), Error> {
    if plan.ci_tool != ToolIdentity::current() {
        return Err(Error::Config(
            "manifest CI producer differs from this executable".into(),
        ));
    }
    let mut ids = BTreeSet::new();
    for node in &plan.nodes {
        crate::config::identifier(&node.id)?;
        if node.output != format!("{}.json", node.id) {
            return Err(Error::Config(format!("invalid task output path: {}", node.output)));
        }
        if node.timeout_seconds == 0 {
            return Err(Error::Config("zero task timeout".into()));
        }
        if !ids.insert(node.id.clone()) {
            return Err(Error::Config(format!("duplicate node {}", node.id)));
        }
    }
    let mut resolved = BTreeSet::new();
    loop {
        let previous = resolved.len();
        for node in &plan.nodes {
            for need in &node.needs {
                if !ids.contains(need) {
                    return Err(Error::Config(format!("unknown prerequisite {need}")));
                }
            }
            if node.needs.iter().all(|n| resolved.contains(n)) {
                resolved.insert(node.id.clone());
            }
        }
        if resolved.len() == plan.nodes.len() {
            return Ok(());
        }
        if previous == resolved.len() {
            return Err(Error::Config("task graph contains a directed cycle".into()));
        }
    }
}
pub(crate) fn bundle(root: &Path) -> Result<String, Error> {
    if crate::runtime_snapshot::path().is_none()
        && fs::symlink_metadata(root.join(".infra/tools/tools.lock.json")).is_ok()
    {
        crate::config::validate_input(root, ".infra/tools/tools.lock.json")?;
    }
    let path = crate::runtime_snapshot::path().unwrap_or_else(|| root.join(".infra/tools/tools.lock.json"));
    if !path.exists() {
        return Ok("development".into());
    }
    let lock: Value = serde_json::from_slice(&fs::read(path)?).map_err(crate::failure::configuration)?;
    lock["bundle_id"]
        .as_str()
        .map(String::from)
        .ok_or_else(crate::failure::missing("snapshot missing bundle_id"))
}
pub(crate) fn revision(root: &Path) -> Result<Option<String>, Error> {
    let metadata = root.join(".git");
    let directory = if metadata.is_file() {
        let pointer = fs::read_to_string(&metadata)?;
        let pointer = pointer
            .trim()
            .strip_prefix("gitdir: ")
            .ok_or_else(crate::failure::missing("invalid .git pointer"))?;
        root.join(pointer)
    } else {
        metadata
    };
    let Some(head) = optional_text(&directory.join("HEAD"))? else {
        return Ok(None);
    };
    let head = head.trim();
    if valid_revision(head) {
        return Ok(Some(head.into()));
    }
    let Some(reference) = head.strip_prefix("ref: ") else {
        return Err(Error::Config("invalid Git HEAD".into()));
    };
    if Path::new(reference).is_absolute()
        || Path::new(reference)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(Error::Config("invalid Git reference".into()));
    }
    let common = if let Some(path) = optional_text(&directory.join("commondir"))? {
        directory.join(path.trim())
    } else {
        directory
    };
    if let Some(value) = optional_text(&common.join(reference))? {
        let value = value.trim();
        if valid_revision(value) {
            return Ok(Some(value.into()));
        }
        return Err(Error::Config("invalid Git revision".into()));
    }
    if let Some(packed) = optional_text(&common.join("packed-refs"))? {
        for line in packed.lines() {
            if let Some((revision, name)) = line.split_once(' ')
                && name == reference
                && valid_revision(revision)
            {
                return Ok(Some(revision.into()));
            }
        }
    }
    Ok(None)
}
fn valid_revision(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn optional_text(path: &Path) -> Result<Option<String>, Error> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::Io(error)),
    }
}

fn pin_revisions(root: &Path, plan: &mut Plan) -> Result<(), Error> {
    if plan.bundle_id == "development" {
        return Ok(());
    }
    let runtime = crate::runtime_snapshot::path();
    if runtime.is_none() {
        crate::config::validate_input(root, ".infra/tools/tools.lock.json")?;
    }
    let path = runtime
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join(".infra/tools/tools.lock.json"));
    let lock: Value = serde_json::from_slice(&fs::read(path)?).map_err(crate::failure::configuration)?;
    if lock["schema"] != 1 || (runtime.is_none() && lock["protocol_major"] != 1) {
        return Err(Error::Config("unsupported snapshot protocol".into()));
    }
    let tools = lock["tools"]
        .as_array()
        .ok_or_else(crate::failure::missing("snapshot tools missing"))?;
    for node in &mut plan.nodes {
        if node.tool == "internal" || node.tool.starts_with("project:") {
            continue;
        }
        let tool = tools
            .iter()
            .find(|t| t["name"] == node.tool)
            .ok_or_else(|| Error::Config(format!("snapshot lacks {}", node.tool)))?;
        let revision = tool["revision"]
            .as_str()
            .ok_or_else(crate::failure::missing("tool revision missing"))?;
        if revision.len() != 40 || !revision.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Config("invalid tool revision".into()));
        }
        node.revision = Some(revision.into());
        node.version = tool["version"].as_str().map(String::from);
    }
    Ok(())
}

fn plan_verify(plan: &mut Plan, config: &Config, selected: Option<&str>, all: bool) -> Result<(), Error> {
    let groups = crate::matrix::groups(config);
    let mut emitted = Vec::new();
    for group in &groups {
        if selected.is_some_and(|id| !group.checks.iter().any(|check| check == id)) {
            continue;
        }
        if !all && !host(&group.scope.os, &group.scope.arch) {
            plan.remote_only.extend(group.checks.iter().cloned());
            continue;
        }
        let doctor = format!("job.{}.doctor-verify", group.scope.id);
        add_doctor(
            plan,
            &doctor,
            "verify",
            &group.profile,
            Some(&group.scope),
            config.timeout_seconds,
        );
        for suite in &group.suites {
            scoped_verify(
                plan,
                &group.profile,
                suite,
                Some(&group.scope),
                &doctor,
                config.timeout_seconds,
            );
        }
        emitted.push(group);
    }
    if selected.is_none() {
        let existing = emitted.iter().find(|group| group.profile == config.primary_profile);
        if let Some(group) = existing {
            let doctor = format!("job.{}.doctor-verify", group.scope.id);
            for suite in ["build", "test"] {
                if !group.suites.iter().any(|current| current == suite) {
                    scoped_verify(
                        plan,
                        &group.profile,
                        suite,
                        Some(&group.scope),
                        &doctor,
                        config.timeout_seconds,
                    );
                }
            }
        } else if !groups.iter().any(|group| group.profile == config.primary_profile) {
            let doctor = "doctor-verify";
            add_doctor(
                plan,
                doctor,
                "verify",
                &config.primary_profile,
                None,
                config.timeout_seconds,
            );
            for suite in ["build", "test"] {
                scoped_verify(
                    plan,
                    &config.primary_profile,
                    suite,
                    None,
                    doctor,
                    config.timeout_seconds,
                );
            }
        }
    }
    Ok(())
}
fn add_doctor(plan: &mut Plan, id: &str, task: &str, profile: &str, scope: Option<&ExecutionScope>, timeout: u64) {
    if plan.nodes.iter().any(|node| node.id == id) {
        return;
    }
    add(
        plan,
        id,
        "tools",
        vec![
            "doctor".into(),
            "--task".into(),
            task.into(),
            "--profile".into(),
            profile.into(),
        ],
        Some(profile.into()),
        Vec::new(),
        timeout,
    );
    if let Some(node) = plan.nodes.last_mut() {
        node.execution = scope.cloned();
    }
}
fn scoped_verify(
    plan: &mut Plan,
    profile: &str,
    suite: &str,
    scope: Option<&ExecutionScope>,
    doctor: &str,
    timeout: u64,
) {
    let prepare = scope.map_or_else(
        || format!("prepare-{profile}"),
        |scope| format!("job.{}.prepare", scope.id),
    );
    let id = scope.map_or_else(
        || format!("verify-{profile}-{suite}"),
        |scope| format!("job.{}.verify-{suite}", scope.id),
    );
    if !plan.nodes.iter().any(|node| node.id == prepare) {
        add(
            plan,
            &prepare,
            "verify",
            vec![
                "prepare".into(),
                "--profile".into(),
                profile.into(),
                "--output".into(),
                format!("@REPORT_ROOT@/{prepare}-build-context.json"),
            ],
            Some(profile.into()),
            vec![doctor.into()],
            timeout,
        );
        if let Some(node) = plan.nodes.last_mut() {
            node.execution = scope.cloned();
        }
    }
    if !plan.nodes.iter().any(|node| node.id == id) {
        add(
            plan,
            &id,
            "verify",
            vec![
                "run".into(),
                "--profile".into(),
                profile.into(),
                "--suite".into(),
                suite.into(),
            ],
            Some(profile.into()),
            vec![doctor.into(), prepare],
            timeout,
        );
        if let Some(node) = plan.nodes.last_mut() {
            node.execution = scope.cloned();
        }
    }
}

fn plan_package(plan: &mut Plan, config: &Config, all: bool) {
    let groups: Vec<_> = crate::matrix::groups(config)
        .into_iter()
        .filter(|group| group.profile == config.primary_profile)
        .collect();
    if groups.is_empty() {
        add_doctor(
            plan,
            "doctor-verify",
            "verify",
            &config.primary_profile,
            None,
            config.timeout_seconds,
        );
        add_verify(
            plan,
            "package",
            &config.primary_profile,
            "package",
            vec!["doctor-verify".into()],
            config.timeout_seconds,
        );
        return;
    }
    for group in groups {
        if !all && !host(&group.scope.os, &group.scope.arch) {
            for check in group.checks {
                if !plan.remote_only.contains(&check) {
                    plan.remote_only.push(check);
                }
            }
            continue;
        }
        let doctor = format!("job.{}.doctor-verify", group.scope.id);
        add_doctor(
            plan,
            &doctor,
            "verify",
            &group.profile,
            Some(&group.scope),
            config.timeout_seconds,
        );
        scoped_verify(
            plan,
            &group.profile,
            "package",
            Some(&group.scope),
            &doctor,
            config.timeout_seconds,
        );
    }
}
