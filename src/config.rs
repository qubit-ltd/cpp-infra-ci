// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Strict project and CI configuration validation.
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

use crate::error::Error;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MatrixEntry {
    pub id: String,
    pub runner: String,
    pub os: String,
    pub arch: String,
    pub profile: String,
    pub suites: Vec<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Hook {
    pub id: String,
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub needs: Vec<String>,
    #[serde(default = "timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "yes")]
    pub required: bool,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    pub schema: u32,
    pub tasks: Vec<String>,
    pub primary_profile: String,
    #[serde(default = "yes")]
    pub local_fail_fast: bool,
    #[serde(default = "one")]
    pub task_jobs: usize,
    #[serde(default = "four")]
    pub matrix_max_parallel: usize,
    #[serde(default = "timeout")]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub require_complete_matrix: bool,
    #[serde(default)]
    pub matrix: Vec<MatrixEntry>,
    #[serde(default)]
    pub hooks: Vec<Hook>,
}
fn yes() -> bool {
    true
}
fn one() -> usize {
    1
}
fn four() -> usize {
    4
}
fn timeout() -> u64 {
    1800
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Profile {
    pub configure: String,
    pub build: String,
    pub test: Option<String>,
    pub conan_context: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Project {
    schema: u32,
    kind: String,
    #[serde(default)]
    source_roots: Vec<String>,
    #[serde(default)]
    test_roots: Vec<String>,
    #[serde(default)]
    generated_roots: Vec<String>,
    #[serde(default)]
    third_party_roots: Vec<String>,
    profiles: BTreeMap<String, Profile>,
}
pub(crate) fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, Error> {
    toml::from_str(&fs::read_to_string(path).map_err(crate::failure::file(path))?).map_err(crate::failure::file(path))
}
pub(crate) fn contained(root: &Path, value: &str) -> Result<PathBuf, Error> {
    let path = Path::new(value);
    if path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(Error::Config(format!("path escapes project: {value}")));
    }
    let mut current = root.to_path_buf();
    for component in path.components() {
        if let Component::Normal(name) = component {
            current.push(name);
            let metadata = fs::symlink_metadata(&current)
                .map_err(|error| Error::Config(format!("invalid project input {}: {error}", current.display())))?;
            if metadata.is_symlink() {
                return Err(Error::Config(format!(
                    "project input path contains symlink: {}",
                    current.display()
                )));
            }
        }
    }
    let canonical = root.join(path).canonicalize()?;
    if !canonical.starts_with(root) {
        return Err(Error::Config(format!("symlink escapes project: {value}")));
    }
    Ok(canonical)
}
pub(crate) fn load(root: &Path) -> Result<(Config, BTreeMap<String, Profile>), Error> {
    validate_input(root, ".infra/project.toml")?;
    validate_input(root, ".infra/ci/config.toml")?;
    if crate::runtime_snapshot::path().is_none()
        && fs::symlink_metadata(root.join(".infra/tools/tools.lock.json")).is_ok()
    {
        validate_input(root, ".infra/tools/tools.lock.json")?;
    }

    let cfg: Config = read_toml(&contained(root, ".infra/ci/config.toml")?)?;
    let project: Project = read_toml(&contained(root, ".infra/project.toml")?)?;
    if cfg.schema != 1 || project.schema != 1 {
        return Err(Error::Config("unsupported schema".into()));
    }
    if !["library", "application", "header-only"].contains(&project.kind.as_str()) {
        return Err(Error::Config("invalid project kind".into()));
    }
    let mut roots = Vec::new();
    for path in project
        .source_roots
        .iter()
        .chain(&project.test_roots)
        .chain(&project.generated_roots)
        .chain(&project.third_party_roots)
    {
        let path = contained(root, path)?;
        if roots
            .iter()
            .any(|p: &PathBuf| p.starts_with(&path) || path.starts_with(p))
        {
            return Err(Error::Config("overlapping project roots".into()));
        }
        roots.push(path);
    }
    if cfg.task_jobs == 0 || cfg.matrix_max_parallel == 0 || cfg.timeout_seconds == 0 || cfg.matrix.len() > 256 {
        return Err(Error::Config(
            "invalid task/matrix resource budget (matrix maximum 256)".into(),
        ));
    }
    let mut seen = BTreeSet::new();
    for task in &cfg.tasks {
        if ![
            "style",
            "dependency",
            "verify",
            "analyze",
            "coverage",
            "package",
            "pages",
        ]
        .contains(&task.as_str())
            || !seen.insert(task)
        {
            return Err(Error::Config(format!("unknown or duplicate task: {task}")));
        }
        let config_tool = if task == "package" { "verify" } else { task };
        if !root.join(format!(".infra/{config_tool}/config.toml")).is_file() {
            return Err(Error::Config(format!("task {task} has no configuration")));
        }
    }
    for task in &cfg.tasks {
        let tool = if task == "package" { "verify" } else { task };
        validate_input(root, &format!(".infra/{tool}/config.toml"))?;
    }
    if cfg.tasks.is_empty() && cfg.hooks.is_empty() {
        return Err(Error::Config("no enabled tasks".into()));
    }
    require_profile(&project.profiles, &cfg.primary_profile)?;
    for name in project.profiles.keys() {
        identifier(name)?;
    }
    for profile in project.profiles.values() {
        if profile.configure.is_empty()
            || profile.build.is_empty()
            || profile.test.as_ref().is_some_and(String::is_empty)
        {
            return Err(Error::Config("empty preset binding".into()));
        }
    }
    let mut ids = BTreeSet::new();
    for entry in &cfg.matrix {
        identifier(&entry.id)?;
        if !ids.insert(&entry.id)
            || entry.runner.is_empty()
            || !["linux", "macos", "windows"].contains(&entry.os.as_str())
            || !["x86_64", "aarch64", "arm64"].contains(&entry.arch.as_str())
        {
            return Err(Error::Config(format!("invalid matrix entry: {}", entry.id)));
        }
        require_profile(&project.profiles, &entry.profile)?;
        let mut suites = BTreeSet::new();
        if entry.suites.is_empty()
            || entry.suites.iter().any(|s| {
                !["build", "test", "headers", "install", "package", "sanitize", "fuzz"].contains(&s.as_str())
                    || !suites.insert(s)
            })
        {
            return Err(Error::Config(format!("invalid suites: {}", entry.id)));
        }
    }
    for hook in &cfg.hooks {
        identifier(&hook.id)?;
        contained(root, &hook.executable)?;
        if hook.timeout_seconds == 0 {
            return Err(Error::Config("zero hook timeout".into()));
        }
    }
    Ok((cfg, project.profiles))
}
pub(crate) fn require_profile(profiles: &BTreeMap<String, Profile>, name: &str) -> Result<(), Error> {
    identifier(name)?;
    if !profiles.contains_key(name) {
        return Err(Error::Config(format!("unknown profile: {name}")));
    }
    Ok(())
}
pub(crate) fn identifier(id: &str) -> Result<(), Error> {
    if id.is_empty() || !id.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c)) || id == "." || id == ".."
    {
        return Err(Error::Config(format!("invalid identifier: {id}")));
    }
    Ok(())
}
pub(crate) fn task_profile(root: &Path, task: &str, profiles: &BTreeMap<String, Profile>) -> Result<String, Error> {
    let relative = format!(".infra/{task}/config.toml");
    validate_input(root, &relative)?;
    let value: toml::Value = read_toml(&root.join(relative))?;
    let name = value
        .get("profile")
        .and_then(toml::Value::as_str)
        .unwrap_or(if task == "analyze" { "analysis" } else { "coverage" });
    require_profile(profiles, name)?;
    Ok(name.into())
}

/// Rejects symbolic configuration selectors before reading or hashing inputs.
/// Configuration files and their project-local ancestors must be ordinary
/// paths; otherwise changing a link could select a different already-hashed
/// policy.
pub(crate) fn validate_input(root: &Path, relative: &str) -> Result<(), Error> {
    let path = Path::new(relative);
    let canonical = contained(root, relative)?;
    if !canonical.is_file() {
        return Err(Error::Config(format!(
            "configuration must be a regular file: {}",
            canonical.display()
        )));
    }
    let resolved = canonical
        .strip_prefix(root)
        .map_err(crate::failure::invalid("configuration escapes project"))?;
    for input in [path, resolved] {
        let normalized = input
            .to_str()
            .ok_or_else(crate::failure::missing("non UTF-8 config input"))?
            .replace('\\', "/");
        let first = normalized.split('/').next().unwrap_or("");
        if [".git", "build", "target"].contains(&first)
            || [".infra/.tmp", ".infra/transactions", ".infra/.transactions"]
                .iter()
                .any(|prefix| normalized == *prefix || normalized.starts_with(&format!("{prefix}/")))
        {
            return Err(Error::Config(format!(
                "configuration input is outside the digest domain: {relative}"
            )));
        }
    }
    if resolved.components().next() != Some(std::path::Component::Normal(std::ffi::OsStr::new(".infra"))) {
        return Err(Error::Config(format!(
            "configuration must resolve inside hashed .infra inputs: {relative}"
        )));
    }
    Ok(())
}
