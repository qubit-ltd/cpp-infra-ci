// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Runner-local build reuse boundaries.
use serde::Deserialize;
use serde::Serialize;
/// A build and its doctor may be reused only within this execution environment.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ExecutionScope {
    /// Stable job group identity.
    pub id: String,
    /// Scheduling runner label.
    pub runner: String,
    /// Runtime operating system.
    pub os: String,
    /// Runtime architecture.
    pub arch: String,
}
