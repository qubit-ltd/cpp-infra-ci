// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Shared canonical path/content hashing.
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use sha2::Digest;
use sha2::Sha256;

use crate::error::Error;

pub(crate) fn bytes(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}
pub(crate) fn file(path: &Path) -> Result<String, Error> {
    Ok(bytes(&fs::read(path)?))
}
pub(crate) fn project(root: &Path, config: bool) -> Result<String, Error> {
    if config {
        crate::config::load(root)?;
    }
    let mut files = Vec::new();
    walk(root, root, config, &mut files)?;
    if !config {
        let excluded = crate::report_paths::current()?;
        files.retain(|path| !excluded.contains(path));
    }
    files.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    let mut hash = Sha256::new();
    for relative in files {
        let data = fs::read(root.join(&relative))?;
        hash.update((relative.len() as u64).to_le_bytes());
        hash.update(relative.as_bytes());
        hash.update((data.len() as u64).to_le_bytes());
        hash.update(data);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn walk(root: &Path, dir: &Path, config: bool, files: &mut Vec<String>) -> Result<(), Error> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path: PathBuf = entry.path();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(crate::failure::configuration)?
            .to_str()
            .ok_or_else(crate::failure::missing("non UTF-8 path"))?
            .replace('\\', "/");
        let first = relative.split('/').next().unwrap_or("");
        if [".git", "build", "target"].contains(&first)
            || (!config && first == ".infra")
            || [".infra/.tmp", ".infra/transactions", ".infra/.transactions"]
                .iter()
                .any(|directory| relative == *directory || relative.starts_with(&format!("{directory}/")))
        {
            continue;
        }
        if kind.is_dir() {
            walk(root, &path, config, files)?;
        } else if kind.is_file()
            && (!config
                || first == ".infra"
                || relative == "CMakeLists.txt"
                || relative == "CMakePresets.json"
                || relative == "conanfile.py"
                || relative == "conanfile.txt"
                || first == "conan")
        {
            files.push(relative);
        }
    }
    Ok(())
}

pub(crate) fn artifact(path: &Path) -> Result<String, Error> {
    if path.is_file() {
        return file(path);
    }
    let mut files = Vec::new();
    artifact_walk(path, path, &mut files)?;
    files.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    let mut hash = Sha256::new();
    for relative in files {
        let content = fs::read(path.join(&relative))?;
        hash.update((relative.len() as u64).to_le_bytes());
        hash.update(relative.as_bytes());
        hash.update((content.len() as u64).to_le_bytes());
        hash.update(content);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn artifact_walk(root: &Path, path: &Path, files: &mut Vec<String>) -> Result<(), Error> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_symlink() {
            return Err(Error::Config("artifact contains symbolic link".into()));
        }
        if kind.is_dir() {
            artifact_walk(root, &path, files)?;
        } else if kind.is_file() {
            files.push(
                path.strip_prefix(root)
                    .map_err(crate::failure::configuration)?
                    .to_str()
                    .ok_or_else(crate::failure::missing("non UTF-8 artifact path"))?
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}
