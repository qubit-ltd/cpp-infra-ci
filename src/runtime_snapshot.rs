// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Invocation-scoped source groups and local executable binding.
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use serde_json::Value;

use crate::digest;
use crate::error::Error;
struct RuntimeSnapshot {
    path: PathBuf,
    digest: String,
    bytes: Vec<u8>,
}
static SNAPSHOT: OnceLock<RuntimeSnapshot> = OnceLock::new();
static COPY_COUNTER: AtomicU64 = AtomicU64::new(0);
pub(crate) fn path() -> Option<PathBuf> {
    SNAPSHOT
        .get()
        .map(|snapshot| snapshot.path.clone())
        .or_else(|| std::env::var_os("CPP_INFRA_RUNTIME_SNAPSHOT").map(PathBuf::from))
}
pub(crate) fn validate(context: Option<&Value>) -> Result<(), Error> {
    let context_path = context.and_then(|context| context["runtime_snapshot"].as_str());
    let selected = context_path.map(PathBuf::from).or_else(path);
    let Some(path) = selected else {
        return Ok(());
    };
    let path = path.canonicalize()?;
    let bytes = fs::read(&path)?;
    let digest = digest::bytes(&bytes);
    if context_path.is_some() && context.is_some_and(|context| context["runtime_snapshot_digest"] != digest) {
        return Err(Error::Config("runtime snapshot digest mismatch".into()));
    }
    let snapshot: Value = serde_json::from_slice(&bytes).map_err(crate::failure::file(&path))?;
    if snapshot["schema"] != 1 || snapshot["bundle_id"].as_str().is_none() {
        return Err(Error::Config("invalid runtime snapshot".into()));
    }
    if context.is_some_and(|context| context["bundle_id"] != snapshot["bundle_id"]) {
        return Err(Error::Config("runtime snapshot bundle mismatch".into()));
    }
    let tools = snapshot["tools"]
        .as_array()
        .ok_or_else(crate::failure::missing("runtime snapshot tools missing"))?;
    let own = tools
        .iter()
        .find(|tool| tool["name"] == "cpp-infra-ci")
        .ok_or_else(crate::failure::missing("runtime snapshot lacks cpp-infra-ci"))?;
    let executable = std::env::current_exe()?.canonicalize()?;
    let declared = Path::new(
        own["binary"]
            .as_str()
            .ok_or_else(crate::failure::missing("runtime tool binary missing"))?,
    )
    .canonicalize()?;
    if own["protocol"] != 1
        || digest::file(&executable)? != own["executable_sha256"]
        || digest::file(&declared)? != own["executable_sha256"]
        || own["version"] != env!("CARGO_PKG_VERSION")
        || own["revision"] != serde_json::json!(option_env!("CPP_INFRA_BUILD_REVISION"))
    {
        return Err(Error::Config(
            "runtime snapshot does not bind this CI executable/version/revision".into(),
        ));
    }
    SNAPSHOT
        .set(RuntimeSnapshot { path, digest, bytes })
        .map_err(crate::failure::invalid("runtime snapshot already initialized"))?;
    Ok(())
}

/// Returns a host-local snapshot path and the digest authenticated at startup.
/// When the invocation belongs to another report root, writes the same bytes to
/// a fresh private file in the target root. Existing files are never replaced;
/// IO or unsafe output ancestors are reported before any child starts.
pub(crate) fn for_report_root(report_root: &Path) -> Result<Option<(PathBuf, String)>, Error> {
    let Some(snapshot) = SNAPSHOT.get() else {
        return Ok(None);
    };
    let root = crate::output_paths::no_symlinks(report_root)?.canonicalize()?;
    if snapshot.path.starts_with(&root) {
        return Ok(Some((snapshot.path.clone(), snapshot.digest.clone())));
    }
    loop {
        let copy = root.join(format!(
            ".cpp-infra-runtime-{}-{}.json",
            std::process::id(),
            COPY_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&copy) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(Error::Io(error)),
        };
        file.write_all(&snapshot.bytes)?;
        file.sync_all()?;
        drop(file);
        crate::output_paths::no_symlinks(&copy)?;
        if digest::file(&copy)? != snapshot.digest {
            return Err(Error::Config("localized runtime snapshot digest mismatch".into()));
        }
        return Ok(Some((copy, snapshot.digest.clone())));
    }
}
