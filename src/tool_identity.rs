// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! The producer identity bound into an immutable execution manifest.
use serde::Deserialize;
use serde::Serialize;
/// Identifies the CI binary which planned and produced wrapper reports.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ToolIdentity {
    /// Executable family name.
    pub name: String,
    /// Build package version.
    pub version: String,
    /// Build source revision, unknown only for development builds.
    pub revision: Option<String>,
}
impl ToolIdentity {
    pub(crate) fn current() -> Self {
        Self {
            name: "cpp-infra-ci".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            revision: option_env!("CPP_INFRA_BUILD_REVISION").map(String::from),
        }
    }
}
