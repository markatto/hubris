// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Architecture-specific syscall stubs.
//!
//! Each architecture module provides the low-level assembly implementations
//! of the syscall stubs. The public API in `lib.rs` calls these stubs.

#[cfg(target_arch = "arm")]
mod arm_m;
#[cfg(target_arch = "arm")]
pub use arm_m::*;
