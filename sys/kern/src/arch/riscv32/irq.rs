// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! RISC-V external interrupt controller abstraction.
//!
//! Different RISC-V chips use different interrupt controllers:
//! - Most: Standard PLIC (Platform-Level Interrupt Controller)
//! - RP2350: Xh3irq (Hazard3 custom interrupt controller)
//! - ESP32-C3: Proprietary interrupt matrix
//!
//! This module provides a trait abstraction so the kernel can use any
//! interrupt controller implementation.

use abi::UsageError;

/// Trait for RISC-V external interrupt controllers.
///
/// The kernel uses this to enable/disable interrupts and handle interrupt
/// dispatch. Implementors handle the chip-specific register details.
pub trait InterruptController {
    /// Enable an external interrupt source.
    fn enable(&self, irq: u32) -> Result<(), UsageError>;

    /// Disable an external interrupt source.
    fn disable(&self, irq: u32) -> Result<(), UsageError>;

    /// Clear pending status for an interrupt.
    fn clear_pending(&self, irq: u32) -> Result<(), UsageError>;

    /// Check if an interrupt is pending.
    fn is_pending(&self, irq: u32) -> Result<bool, UsageError>;

    /// Check if an interrupt is enabled.
    fn is_enabled(&self, irq: u32) -> Result<bool, UsageError>;

    /// Claim the highest-priority pending interrupt.
    ///
    /// Returns the interrupt number if one is pending, or None if no
    /// interrupts are pending. The interrupt remains pending until
    /// `complete()` is called.
    fn claim(&self) -> Option<u32>;

    /// Signal that interrupt handling is complete.
    ///
    /// This must be called after handling an interrupt to allow the
    /// controller to deliver the next interrupt.
    fn complete(&self, irq: u32);
}

// ============================================================================
// PLIC implementation
//
// Standard RISC-V Platform-Level Interrupt Controller, used by:
// - QEMU virt machine
// - SiFive cores
// - Most standard RISC-V implementations
// ============================================================================

/// PLIC controller for standard RISC-V platforms.
///
/// Uses `riscv-peripheral` for register access.
pub struct PlicController {
    /// Base address of the PLIC (chip-specific)
    base: usize,
    /// Context ID for this hart (typically 0 for M-mode on hart 0)
    context: usize,
}

impl PlicController {
    // PLIC register offsets
    const PRIORITY_BASE: usize = 0x0000;
    const PENDING_BASE: usize = 0x1000;
    const ENABLE_BASE: usize = 0x2000;
    const ENABLE_STRIDE: usize = 0x80;
    const THRESHOLD_BASE: usize = 0x20_0000;
    const THRESHOLD_STRIDE: usize = 0x1000;
    const CLAIM_BASE: usize = 0x20_0004;
    const CLAIM_STRIDE: usize = 0x1000;

    /// Create a new PLIC controller.
    ///
    /// # Arguments
    /// * `base` - Base address of the PLIC peripheral
    /// * `context` - Context ID (typically hart_id * 2 for M-mode)
    pub const fn new(base: usize, context: usize) -> Self {
        Self { base, context }
    }

    /// Initialize the PLIC for use.
    ///
    /// Sets threshold to 0 (accept all priorities) and enables M-mode
    /// external interrupts in mie.
    pub fn init(&self) {
        // Set priority threshold to 0 (accept all interrupts)
        let threshold_addr = self.base + Self::THRESHOLD_BASE
            + self.context * Self::THRESHOLD_STRIDE;
        unsafe {
            core::ptr::write_volatile(threshold_addr as *mut u32, 0);
        }

        // Enable machine external interrupts
        unsafe {
            riscv::register::mie::set_mext();
        }
    }

    fn priority_addr(&self, irq: u32) -> usize {
        self.base + Self::PRIORITY_BASE + (irq as usize) * 4
    }

    fn enable_addr(&self, irq: u32) -> (usize, u32) {
        let enable_base = self.base + Self::ENABLE_BASE
            + self.context * Self::ENABLE_STRIDE;
        let reg_offset = (irq / 32) as usize * 4;
        let bit = 1u32 << (irq % 32);
        (enable_base + reg_offset, bit)
    }

    fn pending_addr(&self, irq: u32) -> (usize, u32) {
        let reg_offset = (irq / 32) as usize * 4;
        let bit = 1u32 << (irq % 32);
        (self.base + Self::PENDING_BASE + reg_offset, bit)
    }

    fn claim_addr(&self) -> usize {
        self.base + Self::CLAIM_BASE + self.context * Self::CLAIM_STRIDE
    }
}

impl InterruptController for PlicController {
    fn enable(&self, irq: u32) -> Result<(), UsageError> {
        if irq == 0 {
            return Err(UsageError::NoIrq); // IRQ 0 is reserved
        }

        // Set priority to 1 (lowest non-zero priority)
        let priority_addr = self.priority_addr(irq);
        unsafe {
            core::ptr::write_volatile(priority_addr as *mut u32, 1);
        }

        // Enable the interrupt for this context
        let (addr, bit) = self.enable_addr(irq);
        unsafe {
            let val = core::ptr::read_volatile(addr as *const u32);
            core::ptr::write_volatile(addr as *mut u32, val | bit);
        }

        Ok(())
    }

    fn disable(&self, irq: u32) -> Result<(), UsageError> {
        if irq == 0 {
            return Err(UsageError::NoIrq);
        }

        // Disable the interrupt for this context
        let (addr, bit) = self.enable_addr(irq);
        unsafe {
            let val = core::ptr::read_volatile(addr as *const u32);
            core::ptr::write_volatile(addr as *mut u32, val & !bit);
        }

        Ok(())
    }

    fn clear_pending(&self, irq: u32) -> Result<(), UsageError> {
        if irq == 0 {
            return Err(UsageError::NoIrq);
        }

        // PLIC pending bits are read-only and cleared by claim/complete.
        // We can't directly clear pending, but we can claim and complete
        // if this IRQ is what's pending.
        //
        // For now, just succeed - the interrupt will be cleared when handled.
        Ok(())
    }

    fn is_pending(&self, irq: u32) -> Result<bool, UsageError> {
        if irq == 0 {
            return Err(UsageError::NoIrq);
        }

        let (addr, bit) = self.pending_addr(irq);
        let val = unsafe { core::ptr::read_volatile(addr as *const u32) };
        Ok((val & bit) != 0)
    }

    fn is_enabled(&self, irq: u32) -> Result<bool, UsageError> {
        if irq == 0 {
            return Err(UsageError::NoIrq);
        }

        let (addr, bit) = self.enable_addr(irq);
        let val = unsafe { core::ptr::read_volatile(addr as *const u32) };
        Ok((val & bit) != 0)
    }

    fn claim(&self) -> Option<u32> {
        let addr = self.claim_addr();
        let irq = unsafe { core::ptr::read_volatile(addr as *const u32) };
        if irq == 0 {
            None
        } else {
            Some(irq)
        }
    }

    fn complete(&self, irq: u32) {
        let addr = self.claim_addr();
        unsafe {
            core::ptr::write_volatile(addr as *mut u32, irq);
        }
    }
}

// ============================================================================
// Controller selection
//
// The `Irq` type alias selects which controller implementation to use based
// on the target chip.
// ============================================================================

// QEMU virt machine PLIC configuration
#[cfg(feature = "plic")]
pub static IRQ: PlicController = PlicController::new(
    0x0C00_0000,  // PLIC base address for QEMU virt
    0,            // Context 0 (M-mode, hart 0)
);

// Default to PLIC if no specific controller is selected
#[cfg(not(any(feature = "plic", feature = "xh3irq")))]
pub static IRQ: PlicController = PlicController::new(
    0x0C00_0000,
    0,
);

// TODO: Add Xh3irq implementation for RP2350
// #[cfg(feature = "xh3irq")]
// pub static IRQ: Xh3irqController = Xh3irqController::new();
