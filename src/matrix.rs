// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Coalesce suites only when their complete runner/profile semantics agree.
use crate::config::Config;
use crate::execution_scope::ExecutionScope;

pub(crate) struct Group {
    pub scope: ExecutionScope,
    pub profile: String,
    pub checks: Vec<String>,
    pub suites: Vec<String>,
}
pub(crate) fn groups(config: &Config) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for entry in &config.matrix {
        if let Some(group) = groups.iter_mut().find(|group| {
            group.profile == entry.profile
                && group.scope.runner == entry.runner
                && group.scope.os == entry.os
                && group.scope.arch == entry.arch
        }) {
            group.checks.push(entry.id.clone());
            for suite in &entry.suites {
                if !group.suites.contains(suite) {
                    group.suites.push(suite.clone());
                }
            }
        } else {
            groups.push(Group {
                scope: ExecutionScope {
                    id: format!("matrix-{}", entry.id),
                    runner: entry.runner.clone(),
                    os: entry.os.clone(),
                    arch: entry.arch.clone(),
                },
                profile: entry.profile.clone(),
                checks: vec![entry.id.clone()],
                suites: entry.suites.clone(),
            });
        }
    }
    groups
}
