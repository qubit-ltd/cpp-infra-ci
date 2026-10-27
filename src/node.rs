// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Portable task nodes.
use serde::Deserialize;
use serde::Serialize;

use crate::execution_scope::ExecutionScope;

/// One protocol task, including ordering and exclusive mutable resources.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Node {
    /// Stable task identity within the run.
    pub id: String,
    /// Executable name, or internal summary operation.
    pub tool: String,
    /// Tool-specific arguments, without shared protocol arguments.
    pub args: Vec<String>,
    /// Selected project profile.
    pub profile: Option<String>,
    /// Runner-local scope for build and doctor ownership.
    #[serde(default)]
    pub execution: Option<ExecutionScope>,
    /// Pinned tool version, unknown in direct development mode.
    #[serde(default)]
    pub version: Option<String>,
    /// Prerequisite node identities.
    pub needs: Vec<String>,
    /// Resources which may not overlap in one scheduling wave.
    pub resource_locks: Vec<String>,
    /// Wall-clock process limit.
    pub timeout_seconds: u64,
    /// Whether failure contributes to the final gate.
    pub required: bool,
    /// Relative report path under this run's report root.
    pub output: String,
    /// Pinned executable revision, unknown for development bundles.
    pub revision: Option<String>,
}
