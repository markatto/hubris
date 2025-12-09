// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Simple test task that sends "Hello from UART!\n" to the UART driver.

#![no_std]
#![no_main]

use userlib::*;

task_slot!(UART, uart);

// The UART driver's Write operation number
const OP_WRITE: u16 = 1;

#[export_name = "main"]
fn main() -> ! {
    let uart = UART.get_task_id();
    let msg = b"Hello from UART!\r\n";

    // Send the message to the UART driver
    let (rc, _) = sys_send(
        uart,
        OP_WRITE,
        &[],         // no fixed payload
        &mut [],     // no response expected
        &[Lease::read_only(msg)],
    );

    if rc == 0 {
        // Success! Now loop forever
        loop {
            hl::sleep_for(1000);
        }
    } else {
        // Error - just panic
        panic!("UART write failed: {}", rc);
    }
}

include!(concat!(env!("OUT_DIR"), "/notifications.rs"));
