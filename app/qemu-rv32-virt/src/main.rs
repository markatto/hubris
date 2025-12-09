// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#![no_std]
#![no_main]

// Pull in the riscv-rt runtime - this provides _start, trap handlers, etc.
extern crate riscv_rt;
use riscv_rt::entry;

// QEMU virt machine runs at 10MHz by default
const CYCLES_PER_MS: u32 = 10_000;

#[entry]
fn main() -> ! {
    #[cfg(feature = "klog-semihosting")]
    {
        let _ = riscv_semihosting::hprintln!("Hubris starting on QEMU rv32 virt");
    }

    unsafe { kern::startup::start_kernel(CYCLES_PER_MS) }
}
