// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Validated per-invocation report-file exclusions shared by every child tool.
use std::collections::BTreeSet;
use std::fs;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::RwLock;

use serde_json::Value;

use crate::error::Error;
static EXCLUSIONS: RwLock<Vec<String>> = RwLock::new(Vec::new());
static ROOT: OnceLock<PathBuf> = OnceLock::new();
pub(crate) fn project_root() -> Option<&'static Path> {
    ROOT.get().map(PathBuf::as_path)
}
pub(crate) fn current() -> Result<Vec<String>, Error> {
    EXCLUSIONS
        .read()
        .map(|paths| paths.clone())
        .map_err(crate::failure::invalid("source exclusions lock poisoned"))
}
pub(crate) fn initialize(root: &Path, report: Option<&Path>, context: Option<&Value>) -> Result<(), Error> {
    *EXCLUSIONS
        .write()
        .map_err(crate::failure::invalid("source exclusions lock poisoned"))? = Vec::new();
    ROOT.set(root.to_path_buf())
        .map_err(crate::failure::invalid("source exclusion root already initialized"))?;
    let incoming = if let Some(context) = context {
        parse(context.get("source_excluded_paths"))?
    } else {
        Vec::new()
    };
    merge(root, &incoming)?;
    if let Some(report) = report
        && let Some(relative) = validate_explicit(root, report)?
    {
        merge(root, &[relative])?;
    }
    Ok(())
}
pub(crate) fn parse(value: Option<&Value>) -> Result<Vec<String>, Error> {
    match value {
        None => Ok(Vec::new()),
        Some(value) => serde_json::from_value(value.clone()).map_err(crate::failure::invalid(
            "source_excluded_paths must be an array of paths",
        )),
    }
}
pub(crate) fn validate_list(root: &Path, paths: &[String]) -> Result<(), Error> {
    for path in paths {
        let relative = Path::new(path);
        if path.is_empty()
            || has_drive_prefix(path)
            || path.contains('\\')
            || relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
            || relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
                != *path
            || path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(Error::Config(format!("invalid source exclusion: {path}")));
        }
        let checked = validate_explicit(root, &root.join(relative))?.ok_or_else(crate::failure::missing(
            "source exclusion is outside project or in build outputs",
        ))?;
        if checked != *path {
            return Err(Error::Config(
                "source exclusion is not a canonical relative path".into(),
            ));
        }
    }
    Ok(())
}
pub(crate) fn merge(root: &Path, paths: &[String]) -> Result<(), Error> {
    validate_list(root, paths)?;
    let mut merged: BTreeSet<String> = current()?.into_iter().collect();
    merged.extend(paths.iter().cloned());
    *EXCLUSIONS
        .write()
        .map_err(crate::failure::invalid("source exclusions lock poisoned"))? = merged.into_iter().collect();
    Ok(())
}
pub(crate) fn validate_explicit(root: &Path, path: &Path) -> Result<Option<String>, Error> {
    let path = resolve(path)?;
    let Ok(relative) = path.strip_prefix(root) else {
        return Ok(None);
    };
    let relative = relative
        .to_str()
        .ok_or_else(crate::failure::missing("report path must be UTF-8"))?
        .replace('\\', "/");
    if has_drive_prefix(&relative) {
        return Err(Error::Config(
            "report path has a drive prefix in portable metadata".into(),
        ));
    }
    let first = relative.split('/').next().unwrap_or("");
    if ["build", "target"].contains(&first) {
        return Ok(None);
    }
    if path.extension().and_then(|extension| extension.to_str()) != Some("json")
        || [".git", ".infra", "conan"].contains(&first)
        || ["CMakePresets.json", "CMakeUserPresets.json"].contains(&relative.as_str())
        || first.to_ascii_lowercase().starts_with("readme")
    {
        return Err(Error::Config(format!(
            "report cannot overwrite protected input: {relative}"
        )));
    }
    let project = root.join(".infra/project.toml");
    {
        crate::config::validate_input(root, ".infra/project.toml")?;
        let value: toml::Value = crate::config::read_toml(&project)?;
        for field in ["source_roots", "test_roots", "generated_roots", "third_party_roots"] {
            if let Some(roots) = value.get(field).and_then(toml::Value::as_array) {
                for item in roots {
                    let name = item
                        .as_str()
                        .ok_or_else(crate::failure::missing("project roots must be strings"))?;
                    let declared = crate::config::contained(root, name)?;
                    if path.starts_with(declared) {
                        return Err(Error::Config(format!("report is inside input root: {relative}")));
                    }
                }
            }
        }
    }
    if path.exists() {
        if !path.is_file() {
            return Err(Error::Config("report exclusion must be a file".into()));
        }
        let value: Value = serde_json::from_slice(&fs::read(&path)?)
            .map_err(|_| Error::Config(format!("existing report path is not a public report: {relative}")))?;
        if value["schema"] != 1
            || !value["tool"]["name"]
                .as_str()
                .is_some_and(|name| name.starts_with("cpp-infra-"))
            || value.get("source_digest").is_none()
            || value.get("config_digest").is_none()
            || value.get("status").is_none()
        {
            return Err(Error::Config(format!(
                "refusing to overwrite business JSON: {relative}"
            )));
        }
    }
    Ok(Some(relative))
}
fn resolve(path: &Path) -> Result<PathBuf, Error> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut ancestor = path.as_path();
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        let name = ancestor
            .file_name()
            .ok_or_else(crate::failure::missing("report path has no existing ancestor"))?;
        suffix.push(name.to_os_string());
        ancestor = ancestor
            .parent()
            .ok_or_else(crate::failure::missing("report path has no parent"))?;
    }
    let mut resolved = ancestor.canonicalize()?;
    for component in suffix.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn has_drive_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}
