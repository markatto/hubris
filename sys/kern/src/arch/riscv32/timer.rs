// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! RISC-V timer abstraction.
//!
//! Different RISC-V chips use different timer implementations:
//! - Most: Standard CLINT (Core Local Interruptor) with mtime/mtimecmp
//! - ESP32-C3: Proprietary SYSTIMER peripheral
//!
//! This module provides a trait abstraction so the kernel can use any timer
//! implementation without caring about the details.

/// Trait for RISC-V system timers.
///
/// The kernel uses this to set up periodic tick interrupts for scheduling.
/// Implementors handle the chip-specific register details.
pub trait SysTimer {
    /// Initialize the timer for periodic interrupts.
    ///
    /// `tick_divisor` is the number of timer ticks between interrupts.
    /// After this call, the timer interrupt should fire after `tick_divisor`
    /// ticks, and continue firing periodically.
    fn init(&self, tick_divisor: u32);

    /// Acknowledge the timer interrupt and set up the next tick.
    ///
    /// Called from the timer interrupt handler. This should clear any
    /// pending interrupt state and configure the timer for the next
    /// interrupt (using the `tick_divisor` from `init`).
    fn reset(&self);
}

// Generate CLINT interface for QEMU virt machine using riscv-peripheral.
// The macro creates a CLINT struct with safe register access methods.
//
// TODO: Make the base address configurable per-chip. For now we hardcode
// QEMU virt's address. When we add RP2350 support, we'll need to either:
// - Use cfg attributes to select the right address
// - Or make this runtime configurable
riscv_peripheral::clint_codegen!(
    CLINT,
    base 0x0200_0000,
    mtime_freq 10_000_000  // QEMU virt runs at 10MHz
);

/// Standard CLINT (Core Local Interruptor) timer.
///
/// This is the standard RISC-V timer found on most chips including:
/// - QEMU virt machine
/// - SiFive cores
/// - RP2350 (Raspberry Pi Pico 2)
///
/// Uses `riscv-peripheral` for safe register access. This is a zero-sized
/// type since all state lives in hardware registers.
pub struct ClintTimer;

impl ClintTimer {
    /// Create a new CLINT timer.
    pub const fn new() -> Self {
        Self
    }
}

impl SysTimer for ClintTimer {
    fn init(&self, tick_divisor: u32) {
        let clint = CLINT::new();
        let mtimer = clint.mtimer();
        // Set compare value first, then reset counter.
        // This ensures we don't get a spurious interrupt.
        mtimer.mtimecmp_mhartid().write(tick_divisor as u64);
        mtimer.mtime().write(0);
    }

    fn reset(&self) {
        // Reset mtime to 0 to start counting toward the next interrupt.
        // mtimecmp is already set from init().
        CLINT::new().mtimer().mtime().write(0);
    }
}

// ============================================================================
// Timer selection
//
// The `Timer` type alias selects which timer implementation to use based on
// the target chip. This keeps device-specific configuration out of mod.rs.
// ============================================================================

// For now, all RISC-V targets use CLINT. When we add ESP32-C3 support,
// we'll add cfg attributes here to select SystimerTimer instead.
pub type Timer = ClintTimer;
