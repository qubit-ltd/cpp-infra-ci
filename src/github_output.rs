// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Give each complete runner-local DAG branch exactly one job owner.
use std::collections::BTreeSet;

use serde_json::Value;
use serde_json::json;

use crate::config::Config;
use crate::error::Error;
use crate::plan::Plan;

pub(crate) fn matrix(config: &Config, plan: &Plan) -> Result<Value, Error> {
    let mut owned = BTreeSet::new();
    let mut light = Vec::new();
    let mut analysis = Vec::new();
    let mut hooks = Vec::new();
    let mut pages = Vec::new();
    let mut include = Vec::new();
    for group in crate::matrix::groups(config) {
        let ids: Vec<_> = plan
            .nodes
            .iter()
            .filter(|node| node.execution.as_ref() == Some(&group.scope))
            .map(|node| node.id.clone())
            .collect();
        if ids.is_empty() {
            continue;
        }
        owned.extend(ids.iter().cloned());
        let suites: Vec<_> = plan
            .nodes
            .iter()
            .filter(|node| {
                node.execution.as_ref() == Some(&group.scope)
                    && node.tool == "cpp-infra-verify"
                    && node.args.first().is_some_and(|operation| operation == "run")
            })
            .filter_map(|node| {
                node.args
                    .windows(2)
                    .find(|pair| pair[0] == "--suite")
                    .map(|pair| pair[1].clone())
            })
            .collect();
        include.push(json!({"id":group.scope.id,"checks":group.checks,"runner":group.scope.runner,"os":group.scope.os,"arch":group.scope.arch,"profile":group.profile,"suites":suites,"task_ids":ids.join(",")}));
    }
    for node in &plan.nodes {
        if owned.contains(&node.id) || node.tool == "internal" {
            continue;
        }
        let group = if node.id == "pages" || node.id == "doctor-pages" {
            Some(&mut pages)
        } else if node.id == "doctor-analyze"
            || node.id == "doctor-coverage"
            || node.tool == "cpp-infra-analyze"
            || node.tool == "cpp-infra-coverage"
            || node.id.starts_with("prepare-")
                && node.profile.as_ref().is_some_and(|profile| {
                    plan.nodes.iter().any(|other| {
                        (other.tool == "cpp-infra-analyze" || other.tool == "cpp-infra-coverage")
                            && other.profile.as_ref() == Some(profile)
                            && other.execution.is_none()
                    })
                })
        {
            Some(&mut analysis)
        } else if node.id == "doctor-style"
            || node.id == "doctor-dependency"
            || node.tool == "cpp-infra-style"
            || node.tool == "cpp-infra-dependency"
        {
            Some(&mut light)
        } else if node.tool.starts_with("project:") {
            Some(&mut hooks)
        } else {
            None
        };
        if let Some(group) = group {
            group.push(node.id.clone());
            owned.insert(node.id.clone());
        }
    }
    let remaining: Vec<_> = plan
        .nodes
        .iter()
        .filter(|node| node.tool != "internal" && !owned.contains(&node.id))
        .map(|node| node.id.clone())
        .collect();
    if !remaining.is_empty() {
        include.push(json!({"id":"primary","runner":"ubuntu-latest","os":"linux","arch":"x86_64","profile":plan.primary_profile,"suites":[],"task_ids":remaining.join(",")}));
    }
    if include.len() > 256 {
        return Err(Error::Config(
            "GitHub matrix exceeds 256 jobs after profile expansion".into(),
        ));
    }
    Ok(
        json!({"include":include,"max_parallel":config.matrix_max_parallel,"fail_fast":false,"light_task_ids":light.join(","),"analysis_task_ids":analysis.join(","),"hook_task_ids":hooks.join(","),"pages_task_ids":pages.join(","),"required_task_ids":plan.nodes.iter().filter(|node|node.required).map(|node|node.id.as_str()).collect::<Vec<_>>(),"plan":plan}),
    )
}
