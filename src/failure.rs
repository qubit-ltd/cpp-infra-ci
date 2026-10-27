// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Lazy configuration error constructors shared by protocol readers.
use std::fmt::Display;
use std::path::Path;

use crate::error::Error;

/// Allocates the diagnostic only when an absent value is rejected.
pub(crate) fn missing(message: &str) -> impl FnOnce() -> Error + '_ {
    move || Error::Config(message.into())
}

/// Discards an implementation error while retaining its protocol diagnostic.
pub(crate) fn invalid<E>(message: &str) -> impl FnOnce(E) -> Error + '_ {
    move |_| Error::Config(message.into())
}

/// Classifies a diagnostic without changing its displayed text.
pub(crate) fn configuration<E: Display>(error: E) -> Error {
    Error::Config(error.to_string())
}

/// Formats a fixed contextual prefix only if the operation failed.
pub(crate) fn prefixed<E: Display>(prefix: &str) -> impl FnOnce(E) -> Error + '_ {
    move |error| Error::Config(format!("{prefix}: {error}"))
}

/// Attaches a path only if reading or parsing its contents failed.
pub(crate) fn file<E: Display>(path: &Path) -> impl FnOnce(E) -> Error + '_ {
    move |error| Error::Config(format!("{}: {error}", path.display()))
}
