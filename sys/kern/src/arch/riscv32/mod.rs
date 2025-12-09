// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Architecture support for RISC-V.
//!
//! Currently targets RISC-V RV32IMAC cores running in Machine mode with
//! Physical Memory Protection (PMP) for task isolation.
//!
//! The kernel runs in Machine mode with tasks running in User mode.
//! External interrupts (other than the Machine Timer) are not yet fully
//! supported.

use core::arch::{asm, naked_asm};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::atomic::AtomicExt;

use zerocopy::FromBytes;

use crate::descs::RegionAttributes;
use crate::task;
use crate::time::Timestamp;
use abi::{FaultInfo, FaultSource, InterruptNum, UsageError};

extern crate riscv_rt;

use riscv::interrupt::{Exception, Interrupt, Trap};
use riscv::register;
use riscv::register::mstatus::MPP;

mod irq;
mod timer;

use irq::{InterruptController, IRQ};
use timer::SysTimer;

// Kernel logging - enabled via "klog-semihosting" feature
// Run QEMU with -semihosting flag to see output
#[allow(unused_macros)]
macro_rules! klog {
    ($($tt:tt)*) => {
        #[cfg(feature = "klog-semihosting")]
        { let _ = riscv_semihosting::hprintln!($($tt)*); }
    };
}

macro_rules! uassert {
    ($cond : expr) => {
        if !$cond {
            panic!("Assertion failed!");
        }
    };
}

macro_rules! uassert_eq {
    ($cond1 : expr, $cond2 : expr) => {
        if !($cond1 == $cond2) {
            panic!("Assertion failed!");
        }
    };
}

// Task table access is handled via crate::startup::with_task_table

/// On RISC-V we use a global to record the interrupt table position and extent.
#[no_mangle]
static mut IRQ_TABLE_BASE: Option<NonNull<abi::Interrupt>> = None;
#[no_mangle]
static mut IRQ_TABLE_SIZE: usize = 0;

/// On RISC-V we use a global to record the current task pointer.
#[no_mangle]
static mut CURRENT_TASK_PTR: Option<NonNull<task::Task>> = None;

/// To allow our clock frequency to be easily determined from a debugger.
#[no_mangle]
static mut CLOCK_FREQ_KHZ: u32 = 0;

/// Tick counter for kernel time.
static mut TICKS: u64 = 0;

/// Global timer instance.
/// The concrete type is selected in timer.rs based on the target chip.
static TIMER: timer::Timer = timer::Timer::new();

/// Architecture-specific extension data for memory regions.
///
/// For RISC-V PMP, we pre-compute the pmpaddr and pmpcfg values at build time
/// to speed up context switches.
#[derive(Copy, Clone, Debug, Default)]
#[repr(C)]
pub struct RegionDescExt {
    /// Pre-computed PMP address register value (base >> 2 | size_mask)
    pub pmpaddr: u32,
    /// Pre-computed PMP config byte (R/W/X bits + NAPOT mode)
    pub pmpcfg: u8,
}

/// Compute the PMP register values for a memory region at build time.
pub const fn compute_region_extension_data(
    base: u32,
    size: u32,
    attributes: RegionAttributes,
) -> RegionDescExt {
    // NAPOT encoding requires power-of-two aligned regions
    // pmpaddr = (base >> 2) | ((size >> 1) - 1)
    let pmpaddr = (base >> 2) | ((size >> 1).wrapping_sub(1));

    // Build pmpcfg byte: R=bit0, W=bit1, X=bit2, A=bits[4:3]
    // NAPOT mode = 0b11 in A field
    let mut pmpcfg: u8 = 0b11_000; // NAPOT mode

    if attributes.contains(RegionAttributes::READ) {
        pmpcfg |= 0b001;
    }
    if attributes.contains(RegionAttributes::WRITE) {
        pmpcfg |= 0b010;
    }
    if attributes.contains(RegionAttributes::EXECUTE) {
        pmpcfg |= 0b100;
    }

    RegionDescExt { pmpaddr, pmpcfg }
}

/// RISC-V volatile registers that must be saved across context switches.
#[repr(C)]
#[derive(Clone, Debug, Default, FromBytes)]
pub struct SavedState {
    // NOTE: the following fields must be kept contiguous!
    ra: u32,
    sp: u32,
    gp: u32,
    tp: u32,
    t0: u32,
    t1: u32,
    t2: u32,
    s0: u32,
    s1: u32,
    a0: u32,
    a1: u32,
    a2: u32,
    a3: u32,
    a4: u32,
    a5: u32,
    a6: u32,
    a7: u32,
    s2: u32,
    s3: u32,
    s4: u32,
    s5: u32,
    s6: u32,
    s7: u32,
    s8: u32,
    s9: u32,
    s10: u32,
    s11: u32,
    t3: u32,
    t4: u32,
    t5: u32,
    t6: u32,
    // Additional save value for task program counter
    pc: u32,
    // NOTE: the above fields must be kept contiguous!
}

/// Map the volatile registers to (architecture-independent) syscall argument
/// and return slots.
impl task::ArchState for SavedState {
    fn stack_pointer(&self) -> u32 {
        self.sp
    }

    fn arg0(&self) -> u32 {
        self.a0
    }
    fn arg1(&self) -> u32 {
        self.a1
    }
    fn arg2(&self) -> u32 {
        self.a2
    }
    fn arg3(&self) -> u32 {
        self.a3
    }
    fn arg4(&self) -> u32 {
        self.a4
    }
    fn arg5(&self) -> u32 {
        self.a5
    }
    fn arg6(&self) -> u32 {
        self.a6
    }

    fn syscall_descriptor(&self) -> u32 {
        self.a7
    }

    fn ret0(&mut self, x: u32) {
        self.a0 = x
    }
    fn ret1(&mut self, x: u32) {
        self.a1 = x
    }
    fn ret2(&mut self, x: u32) {
        self.a2 = x
    }
    fn ret3(&mut self, x: u32) {
        self.a3 = x
    }
    fn ret4(&mut self, x: u32) {
        self.a4 = x
    }
    fn ret5(&mut self, x: u32) {
        self.a5 = x
    }
}

/// Records `irqs` as the system-wide interrupt table.
///
/// # Safety
///
/// This stashes a pointer to `irqs` which could alias other references.
pub unsafe fn set_irq_table(irqs: &[abi::Interrupt]) {
    unsafe {
        let prev_table = core::mem::replace(
            &mut IRQ_TABLE_BASE,
            Some(NonNull::new_unchecked(irqs.as_ptr() as *mut abi::Interrupt)),
        );
        uassert_eq!(prev_table, None);
        IRQ_TABLE_SIZE = irqs.len();
    }
}

/// Reinitialize a task to its initial state.
pub fn reinitialize(task: &mut task::Task) {
    *task.save_mut() = SavedState::default();

    // Set the initial stack pointer, ensuring 16-byte stack alignment as per
    // the RISC-V calling convention.
    task.save_mut().sp = task.descriptor().initial_stack;
    uassert!(task.save().sp & 0xF == 0);

    // Set the initial program counter
    task.save_mut().pc = task.descriptor().entry_point;
}

/// Apply memory protection settings for a task using PMP.
pub fn apply_memory_protection(task: &task::Task) {
    let regions = task.region_table();

    // Clear all PMP entries first by setting A=OFF (0) for all configs
    // pmpcfg0 controls pmp0-3, pmpcfg1 controls pmp4-7
    unsafe {
        register::pmpcfg0::write(0);
        register::pmpcfg1::write(0);
    }

    // Build the combined pmpcfg values
    let mut pmpcfg0_val: usize = 0;
    let mut pmpcfg1_val: usize = 0;

    // Safety: PMP register access is inherently unsafe as it controls memory
    // protection. We trust that the task's region table contains valid entries.
    unsafe {
        for (i, region) in regions.iter().enumerate().take(8) {
            let pmpaddr = region.arch_data.pmpaddr as usize;
            let pmpcfg = region.arch_data.pmpcfg as usize;

            match i {
                0 => {
                    register::pmpaddr0::write(pmpaddr);
                    pmpcfg0_val |= pmpcfg;
                }
                1 => {
                    register::pmpaddr1::write(pmpaddr);
                    pmpcfg0_val |= pmpcfg << 8;
                }
                2 => {
                    register::pmpaddr2::write(pmpaddr);
                    pmpcfg0_val |= pmpcfg << 16;
                }
                3 => {
                    register::pmpaddr3::write(pmpaddr);
                    pmpcfg0_val |= pmpcfg << 24;
                }
                4 => {
                    register::pmpaddr4::write(pmpaddr);
                    pmpcfg1_val |= pmpcfg;
                }
                5 => {
                    register::pmpaddr5::write(pmpaddr);
                    pmpcfg1_val |= pmpcfg << 8;
                }
                6 => {
                    register::pmpaddr6::write(pmpaddr);
                    pmpcfg1_val |= pmpcfg << 16;
                }
                7 => {
                    register::pmpaddr7::write(pmpaddr);
                    pmpcfg1_val |= pmpcfg << 24;
                }
                _ => {}
            };
        }

        // Write the combined config values
        register::pmpcfg0::write(pmpcfg0_val);
        register::pmpcfg1::write(pmpcfg1_val);
    }
}

// Trap handler entry point.
//
// We provide our own interrupt vector to handle save/restore of the task on
// entry, overwriting the symbol set up by riscv-rt.
//
// Note: #[repr(align(4))] is not allowed on functions, so we use
// #[link_section] with an aligned section instead.
#[unsafe(naked)]
#[no_mangle]
#[link_section = ".trap.rust"]
#[export_name = "_start_trap"]
pub unsafe extern "C" fn _start_trap() {
    naked_asm!(
        ".balign 4",
        "
        #
        # Store full task status on entry, setting up a0 to point at our
        # current task so that it's passed into our exception handler.
        #
        csrw mscratch, a0
        la a0, CURRENT_TASK_PTR
        lw a0, (a0)
        sw ra,   0*4(a0)
        sw sp,   1*4(a0)
        sw gp,   2*4(a0)
        sw tp,   3*4(a0)
        sw t0,   4*4(a0)
        sw t1,   5*4(a0)
        sw t2,   6*4(a0)
        sw s0,   7*4(a0)
        sw s1,   8*4(a0)
        #sw a0,  9*4(a0)
        sw a1,  10*4(a0)
        sw a2,  11*4(a0)
        sw a3,  12*4(a0)
        sw a4,  13*4(a0)
        sw a5,  14*4(a0)
        sw a6,  15*4(a0)
        sw a7,  16*4(a0)
        sw s2,  17*4(a0)
        sw s3,  18*4(a0)
        sw s4,  19*4(a0)
        sw s5,  20*4(a0)
        sw s6,  21*4(a0)
        sw s7,  22*4(a0)
        sw s8,  23*4(a0)
        sw s9,  24*4(a0)
        sw s10, 25*4(a0)
        sw s11, 26*4(a0)
        sw t3,  27*4(a0)
        sw t4,  28*4(a0)
        sw t5,  29*4(a0)
        sw t6,  30*4(a0)
        csrr a1, mepc
        sw a1,  31*4(a0)    # store mepc for resume
        csrr a1, mscratch
        sw a1, 9*4(a0)      # store a0 itself

        #
        # Jump to our main rust handler
        #
        jal trap_handler

        #
        # On the way out we may have switched to a different task, load
        # everything in and resume (using t6 as it's restored last).
        #
        la t6, CURRENT_TASK_PTR
        lw t6, (t6)

        lw t5,  31*4(t6)     # restore mepc
        csrw mepc, t5

        lw ra,   0*4(t6)
        lw sp,   1*4(t6)
        lw gp,   2*4(t6)
        lw tp,   3*4(t6)
        lw t0,   4*4(t6)
        lw t1,   5*4(t6)
        lw t2,   6*4(t6)
        lw s0,   7*4(t6)
        lw s1,   8*4(t6)
        lw a0,   9*4(t6)
        lw a1,  10*4(t6)
        lw a2,  11*4(t6)
        lw a3,  12*4(t6)
        lw a4,  13*4(t6)
        lw a5,  14*4(t6)
        lw a6,  15*4(t6)
        lw a7,  16*4(t6)
        lw s2,  17*4(t6)
        lw s3,  18*4(t6)
        lw s4,  19*4(t6)
        lw s5,  20*4(t6)
        lw s6,  21*4(t6)
        lw s7,  22*4(t6)
        lw s8,  23*4(t6)
        lw s9,  24*4(t6)
        lw s10, 25*4(t6)
        lw s11, 26*4(t6)
        lw t3,  27*4(t6)
        lw t4,  28*4(t6)
        lw t5,  29*4(t6)
        lw t6,  30*4(t6)

        mret
        ",
    );
}

/// The Rust side of our trap handler after the task's registers have been
/// saved to SavedState.
#[no_mangle]
fn trap_handler(task: &mut task::Task) {
    // Convert from raw trap cause to typed enum
    let raw_trap = register::mcause::read().cause();
    let trap: Result<Trap<Interrupt, Exception>, _> = raw_trap.try_into();

    match trap {
        // Timer interrupt - periodic tick for scheduling
        Ok(Trap::Interrupt(Interrupt::MachineTimer)) => unsafe {
            let ticks = &mut TICKS;
            with_task_table(|tasks| safe_timer_handler(ticks, tasks));
        },
        // External interrupt - hardware IRQs via PLIC
        Ok(Trap::Interrupt(Interrupt::MachineExternal)) => {
            handle_external_interrupt();
        },
        // System Calls.
        Ok(Trap::Exception(Exception::UserEnvCall)) => {
            // Advance program counter past ecall instruction.
            task.save_mut().pc = register::mepc::read() as u32 + 4;
            unsafe {
                asm!(
                    "
                    la a1, CURRENT_TASK_PTR
                    mv a0, a7               # arg0 = syscall number
                    lw a1, (a1)             # arg1 = task ptr
                    jal syscall_entry
                    ",
                    options(nostack),
                );
            }
        }
        // Exceptions. Routed via the most appropriate FaultInfo.
        Ok(Trap::Exception(Exception::IllegalInstruction)) => unsafe {
            handle_fault(task, FaultInfo::IllegalInstruction);
        },
        Ok(Trap::Exception(Exception::LoadFault)) => unsafe {
            handle_fault(
                task,
                FaultInfo::MemoryAccess {
                    address: Some(register::mtval::read() as u32),
                    source: FaultSource::User,
                },
            );
        },
        Ok(Trap::Exception(Exception::StoreFault)) => unsafe {
            handle_fault(
                task,
                FaultInfo::MemoryAccess {
                    address: Some(register::mtval::read() as u32),
                    source: FaultSource::User,
                },
            );
        },
        Ok(Trap::Exception(Exception::InstructionFault)) => unsafe {
            handle_fault(task, FaultInfo::IllegalText);
        },
        _ => {
            // Unknown/unhandled trap
            // TODO: Consider logging via klog! when debugging
        }
    }
}

/// Handle external interrupts from the PLIC.
///
/// This is called when the CPU receives a MachineExternal interrupt.
/// We claim the interrupt from the PLIC, look up the owning task,
/// disable the interrupt (task re-enables via syscall), post a
/// notification to the task, and complete the interrupt.
fn handle_external_interrupt() {
    use irq::InterruptController;

    // Claim the interrupt from the PLIC - returns the IRQ number
    let Some(irq_num) = IRQ.claim() else {
        // Spurious interrupt, no pending IRQ
        return;
    };

    // Look up which task owns this IRQ
    let owner = crate::startup::HUBRIS_IRQ_TASK_LOOKUP
        .get(abi::InterruptNum(irq_num))
        .unwrap_or_else(|| panic!("unhandled IRQ {irq_num}"));

    let switch = with_task_table(|tasks| {
        // Disable the IRQ - task will re-enable via sys_irq_control
        // Ignore errors since the IRQ number is valid (we just claimed it)
        let _ = IRQ.disable(irq_num);

        // Post notification to the owning task
        let n = task::NotificationSet(owner.notification);
        tasks[owner.task as usize].post(n)
    });

    // Complete the interrupt so PLIC can deliver the next one
    IRQ.complete(irq_num);

    // If posting the notification woke a higher-priority task, switch to it
    if switch {
        with_task_table(|tasks| {
            let current = unsafe {
                CURRENT_TASK_PTR
                    .expect("irq before kernel started?")
                    .as_ptr()
            };
            let idx = (current as usize - tasks.as_ptr() as usize)
                / core::mem::size_of::<task::Task>();

            let next = task::select(idx, tasks);
            apply_memory_protection(next);
            unsafe {
                set_current_task(next);
            }
        });
    }
}

#[no_mangle]
unsafe fn handle_fault(task: *mut task::Task, fault: FaultInfo) {
    unsafe {
        with_task_table(|tasks| {
            let idx = (task as usize - tasks.as_ptr() as usize)
                / core::mem::size_of::<task::Task>();

            let next = match task::force_fault(tasks, idx, fault) {
                task::NextTask::Specific(i) => &tasks[i],
                task::NextTask::Other => task::select(idx, tasks),
                task::NextTask::Same => &tasks[idx],
            };

            // Check if we're trying to return to the same faulted task
            let next_ptr = next as *const task::Task;
            let current_ptr = &tasks[idx] as *const task::Task;
            if next_ptr == current_ptr {
                panic!("attempt to return to Task #{} after fault", idx);
            }

            apply_memory_protection(next);
            set_current_task(next);
        });
    }
}

// Timer handling via the SysTimer trait.
//
// The specific timer implementation (CLINT, SYSTIMER, etc.) is selected
// at compile time via the TIMER global. Different chips need different
// timer peripherals - see timer.rs for available implementations.

fn safe_timer_handler(ticks: &mut u64, tasks: &mut [task::Task]) {
    *ticks += 1;
    let now = Timestamp::from(*ticks);

    let switch = task::process_timers(tasks, now);

    if switch != task::NextTask::Same {
        // Need to get the current task index and pick a new task
        let current = unsafe {
            CURRENT_TASK_PTR
                .expect("irq before kernel started?")
                .as_ptr()
        };
        let idx = (current as usize - tasks.as_ptr() as usize)
            / core::mem::size_of::<task::Task>();

        let next = task::select(idx, tasks);
        apply_memory_protection(next);
        unsafe {
            set_current_task(next);
        }
    }

    // Reset timer for next tick
    TIMER.reset();
}

/// Start the first task and begin executing.
#[allow(unused_variables)]
pub fn start_first_task(tick_divisor: u32, task: &task::Task) -> ! {
    klog!("start_first_task: tick_divisor={}", tick_divisor);

    // Set up trap vector - this is required for syscalls and interrupts to work!
    // Our trap handler is _start_trap, defined with link_section ".trap.rust"
    // The address must be 4-byte aligned. Direct mode means all traps go to this address.
    extern "C" {
        fn _start_trap();
    }
    let trap_addr = _start_trap as usize;
    klog!("  mtvec={:#x}", trap_addr);
    unsafe {
        // mtvec format: address[31:2] | mode[1:0]
        // mode 0 = Direct (all traps go to BASE)
        core::arch::asm!("csrw mtvec, {}", in(reg) trap_addr);
    }

    // Configure MPP to switch us to User mode on exit from Machine mode.
    unsafe {
        register::mstatus::set_mpp(MPP::User);
    }

    // Write the initial task program counter.
    let entry = task.save().pc as usize;
    klog!("  entry={:#x} sp={:#x}", entry, task.save().sp);
    // Safety: Writing mepc to set the task entry point before mret
    unsafe { register::mepc::write(entry); }

    // Configure the timer
    unsafe {
        CLOCK_FREQ_KHZ = tick_divisor;
    }
    TIMER.init(tick_divisor);

    // Initialize the interrupt controller (PLIC)
    // This sets threshold to 0 and enables machine external interrupts
    IRQ.init();

    unsafe {
        register::mie::set_mtimer();
        register::mstatus::set_mie();
    }

    // Apply memory protection for the first task
    apply_memory_protection(task);

    klog!("  launching task");

    // Load first task pointer, set its initial stack pointer, and exit out
    // of machine mode, launching the task.
    unsafe {
        CURRENT_TASK_PTR = Some(NonNull::from(task));
        asm!("
            lw sp, ({sp})
            mret",
            sp = in(reg) &task.save().sp,
            options(noreturn)
        );
    }
}

// Use the shared task table access from startup.rs
use crate::startup::with_task_table;

/// Manufacture a shared reference to the interrupt action table from thin air.
pub fn with_irq_table<R>(body: impl FnOnce(&[abi::Interrupt]) -> R) -> R {
    let table = unsafe {
        core::slice::from_raw_parts(
            IRQ_TABLE_BASE.expect("kernel not started").as_ptr(),
            IRQ_TABLE_SIZE,
        )
    };
    body(table)
}

/// Records the address of `task` as the current user task.
///
/// # Safety
///
/// This records a pointer that aliases `task`.
pub unsafe fn set_current_task(task: &task::Task) {
    unsafe {
        CURRENT_TASK_PTR = Some(NonNull::from(task).cast());
    }
}

/// Reads the tick counter.
pub fn now() -> Timestamp {
    Timestamp::from(unsafe { TICKS })
}

/// Disable an external IRQ.
pub fn disable_irq(n: u32, also_clear_pending: bool) -> Result<(), UsageError> {
    IRQ.disable(n)?;
    if also_clear_pending {
        IRQ.clear_pending(n)?;
    }
    Ok(())
}

/// Enable an external IRQ.
pub fn enable_irq(n: u32, also_clear_pending: bool) -> Result<(), UsageError> {
    if also_clear_pending {
        IRQ.clear_pending(n)?;
    }
    IRQ.enable(n)
}

/// Get IRQ status.
pub fn irq_status(n: u32) -> Result<abi::IrqStatus, UsageError> {
    let mut status = abi::IrqStatus::empty();

    if IRQ.is_enabled(n)? {
        status |= abi::IrqStatus::ENABLED;
    }
    if IRQ.is_pending(n)? {
        status |= abi::IrqStatus::PENDING;
    }

    Ok(status)
}

/// Trigger a software IRQ.
///
/// The PLIC (and most RISC-V interrupt controllers) doesn't support
/// software-triggered external interrupts. Unlike ARM's NVIC which has a
/// Set Pending Register (ISPR) to pend any interrupt from software, the PLIC
/// is a pure hardware interrupt controller with no software trigger mechanism.
///
/// This function is only used by `kipc::software_irq`, which is called by the
/// test runner to simulate hardware interrupts for testing. Core Hubris
/// functionality (task restart, fault handling, etc.) does not depend on this.
///
/// Options for future implementation:
/// - Some SoCs may have platform-specific interrupt trigger mechanisms
/// - Xh3irq (RP2350) or ESP32-C3 may have different capabilities
/// - Could bypass interrupt path and directly set task notification bits
#[allow(unused_variables)]
pub fn pend_software_irq(InterruptNum(n): InterruptNum) -> Result<(), UsageError> {
    Err(UsageError::NoIrq)
}

/// Set clock frequency. (Stub - used by startup)
#[allow(unused_variables)]
pub fn set_clock_freq(freq: u32) {
    unsafe {
        CLOCK_FREQ_KHZ = freq;
    }
}

/// Reset the system. (Stub - not yet implemented)
pub fn reset() -> ! {
    // TODO: implement proper system reset for RP2350
    panic!("system reset requested");
}

// RISC-V has atomic operations, so we can use the native swap.
impl AtomicExt for AtomicBool {
    type Primitive = bool;

    #[inline(always)]
    fn swap_polyfill(
        &self,
        value: Self::Primitive,
        ordering: Ordering,
    ) -> Self::Primitive {
        self.swap(value, ordering)
    }
}
