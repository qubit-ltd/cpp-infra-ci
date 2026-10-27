// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Portable, complete execution manifests.
use serde::Deserialize;
use serde::Serialize;

use crate::node::Node;
use crate::tool_identity::ToolIdentity;

/// A plan also serves as the expected-report manifest for distributed jobs.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Plan {
    /// Protocol schema.
    pub schema: u32,
    /// Producer identity required on every CI wrapper report.
    #[serde(default = "ToolIdentity::current")]
    pub ci_tool: ToolIdentity,
    /// Stable snapshot identity.
    pub bundle_id: String,
    /// Shared run identity.
    pub run_id: String,
    /// Source fingerprint including documentation.
    pub source_digest: String,
    /// Validated report files omitted from the shared source fingerprint.
    #[serde(default)]
    pub source_excluded_paths: Vec<String>,
    /// Configuration fingerprint.
    pub config_digest: String,
    /// Git revision, absent for an uncommitted project.
    pub source_revision: Option<String>,
    /// Fingerprint of the complete matrix configuration.
    pub matrix_digest: String,
    /// Ordered logical tasks.
    pub nodes: Vec<Node>,
    /// Matrix entries unavailable on this host.
    pub remote_only: Vec<String>,
    /// Whether this run lacks configured platform coverage.
    pub incomplete_remote_coverage: bool,
    /// Primary profile for tools without a dedicated build profile.
    pub primary_profile: String,
    /// Require every configured platform for a passing final gate.
    pub require_complete_matrix: bool,
    /// Local scheduling budget.
    pub task_jobs: usize,
    /// Stop starting independent tasks after the first local failure.
    pub fail_fast: bool,
}
