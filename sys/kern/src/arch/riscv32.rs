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

use riscv::register;
use riscv::register::mcause::{Exception, Interrupt, Trap};
use riscv::register::mstatus::MPP;

// Kernel logging - stubbed out for now
// TODO: implement proper kernel logging for RISC-V
#[allow(unused_macros)]
macro_rules! klog {
    ($s:expr) => { };
    ($s:expr, $($tt:tt)*) => { };
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

/// On RISC-V we use a global to record the task table position and extent.
#[no_mangle]
static mut TASK_TABLE_BASE: Option<NonNull<task::Task>> = None;
#[no_mangle]
static mut TASK_TABLE_SIZE: usize = 0;

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

/// Records `tasks` as the system-wide task table.
///
/// # Safety
///
/// This stashes a copy of `tasks` without revoking your right to access it,
/// which is a potential aliasing violation if you call `with_task_table`. So
/// don't do that. The normal kernel entry sequences avoid this issue.
pub unsafe fn set_task_table(tasks: &mut [task::Task]) {
    unsafe {
        let prev_task_table = core::mem::replace(
            &mut TASK_TABLE_BASE,
            Some(NonNull::from(&mut tasks[0])),
        );
        uassert_eq!(prev_task_table, None);
        TASK_TABLE_SIZE = tasks.len();
    }
}

/// Records `irqs` as the system-wide interrupt table.
///
/// # Safety
///
/// Same aliasing concerns as `set_task_table`.
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
#[allow(unused_variables)]
pub fn apply_memory_protection(task: &task::Task) {
    for (i, region) in task.region_table().iter().enumerate() {
        let pmpaddr = region.arch_data.pmpaddr as usize;
        let pmpcfg = region.arch_data.pmpcfg as usize;

        match i {
            0 => {
                register::pmpaddr0::write(pmpaddr);
                register::pmpcfg0::write(
                    register::pmpcfg0::read().bits & 0xFFFF_FF00 | pmpcfg,
                );
            }
            1 => {
                register::pmpaddr1::write(pmpaddr);
                register::pmpcfg0::write(
                    register::pmpcfg0::read().bits & 0xFFFF_00FF
                        | (pmpcfg << 8),
                );
            }
            2 => {
                register::pmpaddr2::write(pmpaddr);
                register::pmpcfg0::write(
                    register::pmpcfg0::read().bits & 0xFF00_FFFF
                        | (pmpcfg << 16),
                );
            }
            3 => {
                register::pmpaddr3::write(pmpaddr);
                register::pmpcfg0::write(
                    register::pmpcfg0::read().bits & 0x00FF_FFFF
                        | (pmpcfg << 24),
                );
            }
            4 => {
                register::pmpaddr4::write(pmpaddr);
                register::pmpcfg1::write(
                    register::pmpcfg1::read().bits & 0xFFFF_FF00 | pmpcfg,
                );
            }
            5 => {
                register::pmpaddr5::write(pmpaddr);
                register::pmpcfg1::write(
                    register::pmpcfg1::read().bits & 0xFFFF_00FF
                        | (pmpcfg << 8),
                );
            }
            6 => {
                register::pmpaddr6::write(pmpaddr);
                register::pmpcfg1::write(
                    register::pmpcfg1::read().bits & 0xFF00_FFFF
                        | (pmpcfg << 16),
                );
            }
            7 => {
                register::pmpaddr7::write(pmpaddr);
                register::pmpcfg1::write(
                    register::pmpcfg1::read().bits & 0x00FF_FFFF
                        | (pmpcfg << 24),
                );
            }
            _ => {}
        };
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
    match register::mcause::read().cause() {
        // Interrupts. Only our periodic MachineTimer interrupt is supported.
        Trap::Interrupt(Interrupt::MachineTimer) => unsafe {
            let ticks = &mut TICKS;
            with_task_table(|tasks| safe_timer_handler(ticks, tasks));
        },
        // System Calls.
        Trap::Exception(Exception::UserEnvCall) => {
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
        Trap::Exception(Exception::IllegalInstruction) => unsafe {
            handle_fault(task, FaultInfo::IllegalInstruction);
        },
        Trap::Exception(Exception::LoadFault) => unsafe {
            handle_fault(
                task,
                FaultInfo::MemoryAccess {
                    address: Some(register::mtval::read() as u32),
                    source: FaultSource::User,
                },
            );
        },
        Trap::Exception(Exception::StoreFault) => unsafe {
            handle_fault(
                task,
                FaultInfo::MemoryAccess {
                    address: Some(register::mtval::read() as u32),
                    source: FaultSource::User,
                },
            );
        },
        Trap::Exception(Exception::InstructionFault) => unsafe {
            handle_fault(task, FaultInfo::IllegalText);
        },
        _ => {
            // Unknown trap - log and continue
            // klog!("Unimplemented cause 0b{:b}", register::mcause::read().bits());
        }
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

// Timer handling via CLINT (Core Local Interruptor).
//
// We use riscv-peripheral for safe register access. The CLINT provides:
//   - MSIP (software interrupt) at offset 0x0000
//   - MTIMECMP at offset 0x4000
//   - MTIME at offset 0xBFF8
//
// QEMU virt machine uses standard CLINT at 0x0200_0000.
// TODO: Make this configurable per-chip.

riscv_peripheral::clint_codegen!(
    CLINT,
    base 0x0200_0000,
    mtime_freq 10_000_000  // QEMU virt runs at 10MHz
);

fn set_timer(tick_divisor: u32) {
    let clint = CLINT::new();
    let mtimer = clint.mtimer();
    // Set compare value first, then reset counter
    // Hart 0's MTIMECMP is at offset 0 from the MTIMECMP base
    mtimer.mtimecmp_mhartid().write(tick_divisor as u64);
    mtimer.mtime().write(0);
}

fn safe_timer_handler(ticks: &mut u64, tasks: &mut [task::Task]) {
    *ticks += 1;
    let now = Timestamp::from(*ticks);
    drop(ticks);

    let switch = task::process_timers(tasks, now);

    if switch != task::NextTask::Same {
        unsafe {
            with_task_table(|tasks| {
                let current = CURRENT_TASK_PTR
                    .expect("irq before kernel started?")
                    .as_ptr();
                let idx = (current as usize - tasks.as_ptr() as usize)
                    / core::mem::size_of::<task::Task>();

                let next = task::select(idx, tasks);
                apply_memory_protection(next);
                set_current_task(next);
            });
        }
    }

    // Reset timer for next tick
    CLINT::new().mtimer().mtime().write(0);
}

/// Start the first task and begin executing.
#[allow(unused_variables)]
pub fn start_first_task(tick_divisor: u32, task: &task::Task) -> ! {
    // Configure MPP to switch us to User mode on exit from Machine mode.
    unsafe {
        register::mstatus::set_mpp(MPP::User);
    }

    // Write the initial task program counter.
    register::mepc::write(task.save().pc as *const usize as usize);

    // Configure the timer
    unsafe {
        CLOCK_FREQ_KHZ = tick_divisor;
        set_timer(tick_divisor - 1);
        register::mie::set_mtimer();
        register::mstatus::set_mie();
    }

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

/// Manufacture a mutable/exclusive reference to the task table from thin air.
///
/// # Safety
///
/// Use at kernel entry points only.
pub unsafe fn with_task_table<R>(
    body: impl FnOnce(&mut [task::Task]) -> R,
) -> R {
    unsafe {
        let tasks = core::slice::from_raw_parts_mut(
            TASK_TABLE_BASE.expect("kernel not started").as_mut(),
            TASK_TABLE_SIZE,
        );
        body(tasks)
    }
}

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

/// Disable an IRQ. (Stub - external IRQs not yet implemented)
#[allow(unused_variables)]
pub fn disable_irq(n: u32, also_clear_pending: bool) -> Result<(), UsageError> {
    // TODO: implement IRQ control for RISC-V
    Err(UsageError::NoIrq)
}

/// Enable an IRQ. (Stub - external IRQs not yet implemented)
#[allow(unused_variables)]
pub fn enable_irq(n: u32, also_clear_pending: bool) -> Result<(), UsageError> {
    // TODO: implement IRQ control for RISC-V
    Err(UsageError::NoIrq)
}

/// Get IRQ status. (Stub - external IRQs not yet implemented)
#[allow(unused_variables)]
pub fn irq_status(n: u32) -> Result<abi::IrqStatus, UsageError> {
    // TODO: implement IRQ status for RISC-V
    Err(UsageError::NoIrq)
}

/// Trigger a software IRQ. (Stub - not yet implemented)
#[allow(unused_variables)]
pub fn pend_software_irq(InterruptNum(n): InterruptNum) -> Result<(), UsageError> {
    // TODO: implement software IRQ triggering for RISC-V
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
