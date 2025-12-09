// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Minimal supervisor stub for early bringup.
//!
//! This is a stripped-down jefe that does the absolute minimum: receive
//! fault notifications and restart faulted tasks. No IPC server, no dump
//! support, no external control.

#![no_std]
#![no_main]

use userlib::{kipc, sys_recv_notification};

// The kernel posts bit 0 when any task faults
const FAULT_NOTIFICATION: u32 = 1;

#[export_name = "main"]
fn main() -> ! {
    loop {
        // Wait for fault notification
        sys_recv_notification(FAULT_NOTIFICATION);

        // A task has faulted. Use the kernel's find_faulted_task to locate
        // and restart all faulted tasks.
        let mut search_from = 1; // Start from 1, task 0 is us
        while let Some(faulted) = kipc::find_faulted_task(search_from) {
            kipc::reinit_task(faulted.get(), true);
            search_from = faulted.get() + 1;
        }
    }
}
