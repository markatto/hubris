// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#![no_std]
#![no_main]

extern crate panic_halt;

use riscv_rt::entry;

// RP2350 runs at 150MHz by default
const CYCLES_PER_MS: u32 = 150_000;

#[entry]
fn main() -> ! {
    // TODO: Add RP2350-specific initialization here
    // - Clock configuration
    // - GPIO setup for debugging

    unsafe { kern::startup::start_kernel(CYCLES_PER_MS) }
}
