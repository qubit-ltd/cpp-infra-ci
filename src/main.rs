// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Thin executable adapter for the CI library.
use cpp_infra_ci::run;
fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            error.exit_code()
        }
    };
    std::process::exit(code);
}
