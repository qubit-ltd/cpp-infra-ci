// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Validate an entire export before atomically copying any owned evidence.
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use serde_json::Value;

use crate::digest;
use crate::error::Error;
use crate::output_paths;
use crate::report;
static COPY_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn tasks(input: &Path, output: &Path, ids: &BTreeSet<String>, allow_missing: bool) -> Result<(), Error> {
    let input = input.canonicalize()?;
    let output = output_paths::no_symlinks(output)?;
    if input == output || output.starts_with(&input) || input.starts_with(&output) {
        return Err(Error::Config("export input and destination must be disjoint".into()));
    }
    let mut files = BTreeMap::new();
    let mut directories = BTreeSet::new();
    for id in ids {
        crate::config::identifier(id)?;
        let relative = format!("{id}.json");
        if allow_missing && !input.join(&relative).exists() {
            continue;
        }
        collect(&input, &relative, &mut files, &mut directories)?;
        evidence(
            &input,
            &report::read(&input.join(&relative))?,
            &mut files,
            &mut directories,
            0,
        )?;
    }
    for relative in &directories {
        let destination = output.join(relative);
        output_paths::no_symlinks(&destination)?;
        if destination.exists() && !destination.is_dir() {
            return Err(Error::Config(format!(
                "conflicting exported artifact: {}",
                destination.display()
            )));
        }
    }
    for (relative, source) in &files {
        let destination = output.join(relative);
        output_paths::no_symlinks(&destination)?;
        if destination.exists() && (!destination.is_file() || digest::file(&destination)? != digest::file(source)?) {
            return Err(Error::Config(format!(
                "conflicting exported artifact: {}",
                destination.display()
            )));
        }
    }
    for relative in &directories {
        let directory = output.join(relative);
        output_paths::no_symlinks(&directory)?;
        fs::create_dir_all(&directory)?;
        output_paths::no_symlinks(&directory)?;
    }
    for (relative, source) in files {
        let destination = output.join(relative);
        output_paths::no_symlinks(&destination)?;
        let parent = destination
            .parent()
            .ok_or_else(crate::failure::missing("export file has no parent"))?;
        fs::create_dir_all(parent)?;
        output_paths::no_symlinks(parent)?;
        let temporary = parent.join(format!(
            ".cpp-infra-export-{}-{}",
            std::process::id(),
            COPY_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut output_file = OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        output_file.write_all(&fs::read(source)?)?;
        output_file.sync_all()?;
        drop(output_file);
        fs::rename(temporary, destination)?;
    }
    Ok(())
}
fn evidence(
    input: &Path,
    value: &Value,
    files: &mut BTreeMap<PathBuf, PathBuf>,
    directories: &mut BTreeSet<PathBuf>,
    depth: usize,
) -> Result<(), Error> {
    if depth > 32 {
        return Err(Error::Config("export report nesting exceeds 32".into()));
    }
    for artifact in value["artifacts"]
        .as_array()
        .ok_or_else(crate::failure::missing("report missing artifacts"))?
    {
        let relative = artifact["path"]
            .as_str()
            .ok_or_else(crate::failure::missing("artifact missing path"))?;
        collect(input, relative, files, directories)?;
        if digest::artifact(&input.join(relative))? != artifact["sha256"] {
            return Err(Error::Config(format!("export artifact digest mismatch: {relative}")));
        }
        if artifact["kind"] == "tool-report" {
            evidence(
                input,
                &report::read(&input.join(relative))?,
                files,
                directories,
                depth + 1,
            )?;
        }
    }
    if let Some(commands) = value["commands"].as_array() {
        for command in commands {
            if let Some(log) = command["log"].as_str()
                && input.join(log).exists()
            {
                collect(input, log, files, directories)?;
            }
        }
    }
    Ok(())
}
fn collect(
    input: &Path,
    relative: &str,
    files: &mut BTreeMap<PathBuf, PathBuf>,
    directories: &mut BTreeSet<PathBuf>,
) -> Result<(), Error> {
    let relative = Path::new(relative);
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative.components().any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(Error::Config("export path escapes input".into()));
    }
    let source = input.join(relative);
    output_paths::no_symlinks(&source)?;
    if !source.canonicalize()?.starts_with(input) {
        return Err(Error::Config("export source outside input".into()));
    }
    for parent in relative.ancestors().skip(1) {
        if !parent.as_os_str().is_empty() {
            directories.insert(parent.to_path_buf());
        }
    }
    if source.is_dir() {
        directories.insert(relative.to_path_buf());
        for entry in fs::read_dir(&source)? {
            let child = relative.join(entry?.file_name());
            collect(
                input,
                child
                    .to_str()
                    .ok_or_else(crate::failure::missing("non UTF-8 export path"))?,
                files,
                directories,
            )?;
        }
    } else if source.is_file() {
        files.insert(relative.to_path_buf(), source);
    } else {
        return Err(Error::Config("export source is not a regular file".into()));
    }
    Ok(())
}
