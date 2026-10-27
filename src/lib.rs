// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Deterministic C++ quality task planning, execution, and report aggregation.
mod cancellation;
mod cli;
mod config;
mod digest;
pub mod error;
pub mod execution_scope;
mod export;
mod failure;
mod github_output;
mod matrix;
pub mod node;
mod output_paths;
pub mod plan;
mod planner;
mod report;
mod report_paths;
mod runtime_snapshot;
mod scheduler;
pub mod tool_identity;
pub use cli::run;
