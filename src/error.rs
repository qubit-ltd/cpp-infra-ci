// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Classified command failures.
use thiserror::Error;

/// Errors retain the protocol exit category and actionable context.
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid configuration, identity, or protocol.
    #[error("{0}")]
    Config(String),
    /// A required gate failed.
    #[error("{0}")]
    Gate(String),
    /// Filesystem or process infrastructure failed.
    #[error("{0}")]
    Io(#[from] std::io::Error),
}
impl Error {
    /// Returns the shared CLI exit category.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Config(_) => 2,
            Self::Gate(_) => 1,
            Self::Io(_) => 3,
        }
    }
}
