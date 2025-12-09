// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A driver for the NS16550A UART.
//!
//! This is a standard UART found on many platforms including the QEMU virt
//! machine. Currently implements polled TX only.
//!
//! # IPC protocol
//!
//! ## `write` (1)
//!
//! Sends the contents of lease #0. Returns when completed.
//!
//! # Configuration
//!
//! The UART base address is provided via the `uart0` extern-region in the
//! task's app.toml configuration. The linker provides the `__REGION_UART0_BASE`
//! symbol.

#![no_std]
#![no_main]

use ns16550a::{Divisor, Uart, WordLength, StopBits, ParityBit, ParitySelect, StickParity, Break, DMAMode};
use userlib::*;

// UART base address - provided by linker from extern-regions config
extern "C" {
    static __REGION_UART0_BASE: [u8; 0];
}

#[derive(Copy, Clone, Debug, num_derive::FromPrimitive)]
enum Operation {
    Write = 1,
}

#[repr(u32)]
enum ResponseCode {
    BadArg = 2,
    Busy = 3,
}

impl From<ResponseCode> for u32 {
    fn from(rc: ResponseCode) -> Self {
        rc as u32
    }
}

struct Transmit {
    caller: hl::Caller<()>,
    len: usize,
    pos: usize,
}

#[export_name = "main"]
fn main() -> ! {
    // Get UART base address from linker symbol (set via extern-regions)
    let uart_base = &raw const __REGION_UART0_BASE as usize;

    // Create and initialize the UART
    let uart = Uart::new(uart_base);
    uart.init(
        WordLength::EIGHT,
        StopBits::ONE,
        ParityBit::DISABLE,
        ParitySelect::EVEN,
        StickParity::DISABLE,
        Break::DISABLE,
        DMAMode::MODE0,
        Divisor::BAUD115200,
    );

    // Test: write directly to UART on startup (bypass ns16550a crate)
    let thr = uart_base as *mut u8;
    for &b in b"[UART driver started]\r\n" {
        unsafe {
            core::ptr::write_volatile(thr, b);
        }
    }

    // Field messages - polled mode, no interrupts needed
    let mut tx: Option<Transmit> = None;

    loop {
        // If we have an active transmission, continue it
        if tx.is_some() {
            step_transmit(&uart, &mut tx);
            // Small yield to not starve other tasks during long writes
            if tx.is_some() {
                hl::sleep_for(1);
                continue;
            }
        }

        // Wait for incoming messages
        hl::recv(
            &mut [],
            0, // No notifications (polled mode)
            &mut tx,
            |_txref, _bits| {
                // No notification handler needed for polled mode
            },
            |txref, op, msg| match op {
                Operation::Write => {
                    let ((), caller) =
                        msg.fixed_with_leases(1).ok_or(ResponseCode::BadArg)?;

                    if txref.is_some() {
                        return Err(ResponseCode::Busy);
                    }

                    let borrow = caller.borrow(0);
                    let info = borrow.info().ok_or(ResponseCode::BadArg)?;

                    if !info.attributes.contains(LeaseAttributes::READ) {
                        return Err(ResponseCode::BadArg);
                    }

                    *txref = Some(Transmit {
                        caller,
                        pos: 0,
                        len: info.len,
                    });

                    Ok(())
                }
            },
        );
    }
}

fn step_transmit(uart: &Uart, tx: &mut Option<Transmit>) {
    let txs = if let Some(txs) = tx { txs } else { return };

    // Try to send as many bytes as possible
    while txs.pos < txs.len {
        if let Some(byte) = txs.caller.borrow(0).read_at::<u8>(txs.pos) {
            // Try to push byte - returns None if UART busy
            if uart.put(byte).is_some() {
                txs.pos += 1;
            } else {
                // UART TX busy, try again later
                return;
            }
        } else {
            // Failed to read from lease - error
            tx.take().unwrap().caller.reply_fail(ResponseCode::BadArg);
            return;
        }
    }

    // Transmission complete
    tx.take().unwrap().caller.reply(());
}

include!(concat!(env!("OUT_DIR"), "/notifications.rs"));
