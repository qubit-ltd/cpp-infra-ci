// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Cooperative root cancellation propagated to every process group.
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use crate::error::Error;
static CANCELLED: AtomicBool = AtomicBool::new(false);
pub(crate) fn install() -> Result<(), Error> {
    ctrlc::set_handler(|| CANCELLED.store(true, Ordering::SeqCst))
        .map_err(crate::failure::prefixed("cannot install cancellation handler"))
}
pub(crate) fn requested() -> bool {
    CANCELLED.load(Ordering::SeqCst)
}
