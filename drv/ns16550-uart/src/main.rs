// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A driver for the NS16550A UART.
//!
//! This is a standard UART found on many platforms including the QEMU virt
//! machine. Implements interrupt-driven TX and RX.
//!
//! # IPC protocol
//!
//! ## `write` (1)
//!
//! Sends the contents of lease #0. Returns when completed.
//!
//! ## `read` (2)
//!
//! Reads up to the size of lease #0 into the lease. Returns the number of
//! bytes actually read (may be less than requested if not enough data is
//! available).
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

// NS16550 register offsets
const IER_OFFSET: usize = 1;  // Interrupt Enable Register
const IIR_OFFSET: usize = 2;  // Interrupt Identification Register
const LSR_OFFSET: usize = 5;  // Line Status Register

// IER bits
const IER_RX_AVAIL: u8 = 0x01;  // Enable Received Data Available interrupt
const IER_TX_EMPTY: u8 = 0x02;  // Enable Transmitter Holding Register Empty interrupt

// LSR bits
const LSR_DATA_READY: u8 = 0x01;  // Data Ready
const LSR_THR_EMPTY: u8 = 0x20;  // Transmitter Holding Register Empty

// RX buffer size
const RX_BUF_SIZE: usize = 64;

#[derive(Copy, Clone, Debug, num_derive::FromPrimitive)]
enum Operation {
    Write = 1,
    Read = 2,
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

/// Simple ring buffer for RX data
struct RxBuffer {
    buf: [u8; RX_BUF_SIZE],
    head: usize,
    tail: usize,
}

impl RxBuffer {
    const fn new() -> Self {
        Self {
            buf: [0; RX_BUF_SIZE],
            head: 0,
            tail: 0,
        }
    }

    fn push(&mut self, byte: u8) -> bool {
        let next = (self.head + 1) % RX_BUF_SIZE;
        if next == self.tail {
            false // Buffer full
        } else {
            self.buf[self.head] = byte;
            self.head = next;
            true
        }
    }

    fn pop(&mut self) -> Option<u8> {
        if self.head == self.tail {
            None // Buffer empty
        } else {
            let byte = self.buf[self.tail];
            self.tail = (self.tail + 1) % RX_BUF_SIZE;
            Some(byte)
        }
    }

    fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    fn len(&self) -> usize {
        if self.head >= self.tail {
            self.head - self.tail
        } else {
            RX_BUF_SIZE - self.tail + self.head
        }
    }
}

struct UartRegs {
    base: usize,
}

impl UartRegs {
    fn new(base: usize) -> Self {
        Self { base }
    }

    fn read_lsr(&self) -> u8 {
        unsafe { core::ptr::read_volatile((self.base + LSR_OFFSET) as *const u8) }
    }

    fn read_data(&self) -> u8 {
        unsafe { core::ptr::read_volatile(self.base as *const u8) }
    }

    fn write_data(&self, byte: u8) {
        unsafe { core::ptr::write_volatile(self.base as *mut u8, byte) }
    }

    fn write_ier(&self, val: u8) {
        unsafe { core::ptr::write_volatile((self.base + IER_OFFSET) as *mut u8, val) }
    }

    fn read_ier(&self) -> u8 {
        unsafe { core::ptr::read_volatile((self.base + IER_OFFSET) as *const u8) }
    }

    fn enable_rx_interrupt(&self) {
        let ier = self.read_ier();
        self.write_ier(ier | IER_RX_AVAIL);
    }

    fn enable_tx_interrupt(&self) {
        let ier = self.read_ier();
        self.write_ier(ier | IER_TX_EMPTY);
    }

    fn disable_tx_interrupt(&self) {
        let ier = self.read_ier();
        self.write_ier(ier & !IER_TX_EMPTY);
    }

    fn data_ready(&self) -> bool {
        (self.read_lsr() & LSR_DATA_READY) != 0
    }

    fn thr_empty(&self) -> bool {
        (self.read_lsr() & LSR_THR_EMPTY) != 0
    }
}

#[export_name = "main"]
fn main() -> ! {
    // Get UART base address from linker symbol (set via extern-regions)
    let uart_base = &raw const __REGION_UART0_BASE as usize;

    // Create and initialize the UART using the ns16550a crate
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

    // Create our register accessor for interrupt control
    let regs = UartRegs::new(uart_base);

    // Enable RX interrupts (we always want to buffer incoming data)
    regs.enable_rx_interrupt();

    // Enable the IRQ at the kernel level
    sys_irq_control(notifications::UART_IRQ_MASK, true);

    // State
    let mut tx: Option<Transmit> = None;
    let mut rx_buf = RxBuffer::new();

    loop {
        hl::recv(
            &mut [],
            notifications::UART_IRQ_MASK,
            &mut (&mut tx, &mut rx_buf),
            // Notification handler
            |(txref, rxbuf), bits| {
                if bits & notifications::UART_IRQ_MASK != 0 {
                    // Handle UART interrupt

                    // Drain RX FIFO into our buffer
                    while regs.data_ready() {
                        let byte = regs.read_data();
                        rxbuf.push(byte); // Ignore overflow for now
                    }

                    // Handle TX if we have an active transmission
                    // Send as many bytes as the FIFO can hold to maximize throughput
                    // The NS16550 has a 16-byte TX FIFO
                    if let Some(txs) = txref.as_mut() {
                        while regs.thr_empty() && txs.pos < txs.len {
                            if let Some(byte) = txs.caller.borrow(0).read_at::<u8>(txs.pos) {
                                regs.write_data(byte);
                                txs.pos += 1;
                            } else {
                                // Failed to read from lease
                                regs.disable_tx_interrupt();
                                txref.take().unwrap().caller.reply_fail(ResponseCode::BadArg);
                                break;
                            }
                        }
                        // Check if transmission is complete
                        if txref.as_ref().map(|t| t.pos >= t.len).unwrap_or(false) {
                            regs.disable_tx_interrupt();
                            txref.take().unwrap().caller.reply(());
                        }
                    }

                    // Re-enable the IRQ
                    sys_irq_control(notifications::UART_IRQ_MASK, true);
                }
            },
            // Message handler
            |(txref, rxbuf), op, msg| match op {
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

                    if info.len == 0 {
                        // Nothing to send
                        caller.reply(());
                        return Ok(());
                    }

                    // Send the first byte immediately to kick off TX
                    // The NS16550 TX empty interrupt fires when THR transitions
                    // from full to empty, so we need to prime it
                    let first_byte = caller.borrow(0).read_at::<u8>(0)
                        .ok_or(ResponseCode::BadArg)?;
                    regs.write_data(first_byte);

                    if info.len == 1 {
                        // Only one byte, already sent
                        caller.reply(());
                        return Ok(());
                    }

                    // More bytes to send - set up transmission state
                    **txref = Some(Transmit {
                        caller,
                        pos: 1, // Already sent first byte
                        len: info.len,
                    });

                    // Enable TX interrupt to continue sending
                    regs.enable_tx_interrupt();

                    Ok(())
                }
                Operation::Read => {
                    let ((), caller) =
                        msg.fixed_with_leases(1).ok_or(ResponseCode::BadArg)?;

                    let borrow = caller.borrow(0);
                    let info = borrow.info().ok_or(ResponseCode::BadArg)?;

                    if !info.attributes.contains(LeaseAttributes::WRITE) {
                        return Err(ResponseCode::BadArg);
                    }

                    // Read available data into the lease
                    let mut count = 0usize;
                    while count < info.len {
                        if let Some(byte) = rxbuf.pop() {
                            // Write to lease
                            if caller.borrow(0).write_at(count, byte).is_none() {
                                break;
                            }
                            count += 1;
                        } else {
                            break; // No more data
                        }
                    }

                    caller.reply(count as u32);
                    Ok(())
                }
            },
        );
    }
}

include!(concat!(env!("OUT_DIR"), "/notifications.rs"));
