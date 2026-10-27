// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Preflight ownership and ancestor checks before any orchestration write.
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde_json::Value;

use crate::error::Error;
use crate::plan::Plan;
use crate::report;

pub(crate) fn no_symlinks(path: &Path) -> Result<PathBuf, Error> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut current = PathBuf::new();
    for component in absolute.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(Error::Config("output path contains parent traversal".into()));
        }
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_symlink() => {
                return Err(Error::Config(format!(
                    "output path contains symbolic link: {}",
                    current.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::Io(error)),
        }
    }
    let mut ancestor = absolute.as_path();
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        suffix.push(
            ancestor
                .file_name()
                .ok_or_else(crate::failure::missing("output has no file component"))?
                .to_os_string(),
        );
        ancestor = ancestor
            .parent()
            .ok_or_else(crate::failure::missing("output has no existing ancestor"))?;
    }
    let mut normalized = ancestor.canonicalize()?;
    for name in suffix.into_iter().rev() {
        normalized.push(name);
    }
    Ok(normalized)
}
pub(crate) fn run_root(root: &Path, output: &Path, plan: &Plan) -> Result<(), Error> {
    crate::config::identifier(&plan.run_id)?;
    let expected = root.join("build/infra-runs").join(&plan.run_id);
    let normalized = no_symlinks(output)?;
    if normalized != expected {
        return Err(Error::Config(
            "execute output must be the canonical project/build/infra-runs/<run_id> directory".into(),
        ));
    }
    no_symlinks(output)?;
    if output.exists() && !output.is_dir() {
        return Err(Error::Config("run output is not a directory".into()));
    }
    let manifest = output.join("run-manifest.json");
    no_symlinks(&manifest)?;
    if manifest.exists() {
        let existing: Plan = serde_json::from_slice(&fs::read(&manifest)?).map_err(crate::failure::invalid(
            "refusing to overwrite an unmanaged run-manifest.json",
        ))?;
        if existing.run_id != plan.run_id
            || existing.bundle_id != plan.bundle_id
            || existing.source_digest != plan.source_digest
            || existing.config_digest != plan.config_digest
            || existing.matrix_digest != plan.matrix_digest
        {
            return Err(Error::Config(
                "existing run manifest belongs to another invocation".into(),
            ));
        }
    }
    for node in &plan.nodes {
        let wrapper = output.join(&node.output);
        no_symlinks(&wrapper)?;
        if wrapper.exists() {
            owned(&wrapper, plan, &node.id, Some("cpp-infra-ci"))?;
        }
        let context = output.join(format!("{}-context.json", node.id));
        no_symlinks(&context)?;
        if context.exists() {
            owned(&context, plan, &node.id, None)?;
        }
        let child = output.join(format!("{}-tool.json", node.id));
        no_symlinks(&child)?;
        if child.exists() {
            owned(&child, plan, &node.id, Some(&node.tool))?;
        }
        for relative in [format!("{}.log", node.id), format!("{}-build-context.json", node.id)] {
            let path = output.join(relative);
            no_symlinks(&path)?;
            if path.exists() && !context.exists() && !wrapper.exists() {
                return Err(Error::Config(format!(
                    "refusing to overwrite unmanaged run output: {}",
                    path.display()
                )));
            }
        }
    }
    Ok(())
}
fn owned(path: &Path, plan: &Plan, id: &str, tool: Option<&str>) -> Result<Value, Error> {
    let value = report::read(path)
        .map_err(|_| Error::Config(format!("refusing to overwrite unmanaged run file: {}", path.display())))?;
    if value["schema"] != 1
        || value["run_id"] != plan.run_id
        || value["task_id"] != id
        || value["bundle_id"] != plan.bundle_id
        || tool.is_some_and(|tool| value["tool"]["name"] != tool)
    {
        return Err(Error::Config(format!(
            "run output identity mismatch: {}",
            path.display()
        )));
    }
    Ok(value)
}
