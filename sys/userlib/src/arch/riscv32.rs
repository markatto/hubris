// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! RISC-V architecture support for userlib.
//!
//! This module contains the syscall stubs and entry point for RISC-V
//! processors. The stubs use inline assembly to perform syscalls via ECALL.
//!
//! Currently targets RV32IMAC (e.g., RP2350 Hazard3 cores).

use core::arch;

use crate::{
    BorrowReadArgs, BorrowWriteArgs, RawBorrowInfo, RawRecvMessage,
    RawTimerState, RcLen, SendArgs, Sysnum,
};

/// This is the entry point for the task, invoked by the kernel. Its job is to
/// set up our memory before jumping to user-defined `main`.
#[doc(hidden)]
#[no_mangle]
#[link_section = ".text.start"]
#[unsafe(naked)]
pub unsafe extern "C" fn _start() -> ! {
    // Provided by the user program:
    extern "Rust" {
        fn main() -> !;
    }

    arch::naked_asm!("
        # Copy data initialization image into data section.
        la t0, __edata      # upper bound in t0
        la t1, __sidata     # source in t1
        la t2, __sdata      # dest in t2
        j 1f

    2:  lw s3, (t1)
        addi t1, t1, 4
        sw s3, (t2)
        addi t2, t2, 4

    1:  bne t2, t0, 2b

        # Zero BSS
        la t0, __ebss       # upper bound in t0
        la t1, __sbss       # base in t1
        j 1f

    2:  sw zero, (t1)
        addi t1, t1, 4

    1:  bne t1, t0, 2b

        # Jump to main. We use tail call since main is noreturn.
        tail {main}
        ",
        main = sym main,
    )
}

/// Core implementation of the SEND syscall.
///
/// See the note on syscall stubs at the top of the lib module for rationale.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn sys_send_stub(
    _args: &mut SendArgs<'_>,
) -> RcLen {
    arch::naked_asm!("
        # Load in args from the struct.
        lw a6, 6*4(a0)
        lw a5, 5*4(a0)
        lw a4, 4*4(a0)
        lw a3, 3*4(a0)
        lw a2, 2*4(a0)
        lw a1, 1*4(a0)
        lw a0, 0*4(a0)

        # Load the constant syscall number.
        li a7, {sysnum}

        # To the kernel!
        ecall

        # Results are placed into the correct registers by the kernel, we can
        # just return now.
        ret
        ",
        sysnum = const Sysnum::Send as u32,
    )
}

/// Core implementation of the RECV syscall.
///
/// See the note on syscall stubs at the top of the lib module for rationale.
#[unsafe(naked)]
#[must_use]
pub(crate) unsafe extern "C" fn sys_recv_stub(
    _buffer_ptr: *mut u8,
    _buffer_len: usize,
    _notification_mask: u32,
    _specific_sender: u32,
    _out: *mut RawRecvMessage,
) -> u32 {
    arch::naked_asm!("
        # Preserve output buffer pointer in callee-save register, ensuring
        # it is saved on the stack, which is kept 16-byte aligned.
        addi sp, sp, -16
        sw s2, 0(sp)
        mv s2, a4

        # Load the constant syscall number.
        li a7, {sysnum}

        # To the kernel!
        ecall

        # Write all the results out into the raw output buffer.
        sw a1, 0*4(s2)
        sw a2, 1*4(s2)
        sw a3, 2*4(s2)
        sw a4, 3*4(s2)
        sw a5, 4*4(s2)

        # Restore callee-save register and stack pointer and return.
        lw s2, 0(sp)
        addi sp, sp, 16
        ret
        ",
        sysnum = const Sysnum::Recv as u32,
    )
}

/// Core implementation of the REPLY syscall.
///
/// See the note on syscall stubs at the top of the lib module for rationale.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn sys_reply_stub(
    _peer: u32,
    _code: u32,
    _message_ptr: *const u8,
    _message_len: usize,
) {
    arch::naked_asm!("
        # Load the constant syscall number.
        li a7, {sysnum}

        # To the kernel!
        ecall

        # No results, we can just return now.
        ret
        ",
        sysnum = const Sysnum::Reply as u32,
    )
}

/// Core implementation of the SET_TIMER syscall.
///
/// See the note on syscall stubs at the top of the lib module for rationale.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn sys_set_timer_stub(
    _set_timer: u32,
    _deadline_lo: u32,
    _deadline_hi: u32,
    _notification: u32,
) {
    arch::naked_asm!("
        # Load the constant syscall number.
        li a7, {sysnum}

        # To the kernel!
        ecall

        # No results, we can just return now.
        ret
        ",
        sysnum = const Sysnum::SetTimer as u32,
    )
}

/// Core implementation of the BORROW_READ syscall.
///
/// See the note on syscall stubs at the top of the lib module for rationale.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn sys_borrow_read_stub(
    _args: *mut BorrowReadArgs,
) -> RcLen {
    arch::naked_asm!("
        # Move register arguments into place, in reverse order so that a0 is
        # loaded last when we're finished with it.
        lw a4, 4*4(a0)
        lw a3, 3*4(a0)
        lw a2, 2*4(a0)
        lw a1, 1*4(a0)
        lw a0, 0*4(a0)

        # Load the constant syscall number.
        li a7, {sysnum}

        # To the kernel!
        ecall

        # Results will be placed into return registers by the kernel, we can
        # just return now.
        ret
        ",
        sysnum = const Sysnum::BorrowRead as u32,
    )
}

/// Core implementation of the BORROW_WRITE syscall.
///
/// See the note on syscall stubs at the top of the lib module for rationale.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn sys_borrow_write_stub(
    _args: *mut BorrowWriteArgs,
) -> RcLen {
    arch::naked_asm!("
        # Move register arguments into place, in reverse order so that a0 is
        # loaded last when we're finished with it.
        lw a4, 4*4(a0)
        lw a3, 3*4(a0)
        lw a2, 2*4(a0)
        lw a1, 1*4(a0)
        lw a0, 0*4(a0)

        # Load the constant syscall number.
        li a7, {sysnum}

        # To the kernel!
        ecall

        # Results are placed into the correct registers by the kernel, we can
        # just return now.
        ret
        ",
        sysnum = const Sysnum::BorrowWrite as u32,
    )
}

/// Core implementation of the BORROW_INFO syscall.
///
/// See the note on syscall stubs at the top of the lib module for rationale.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn sys_borrow_info_stub(
    _lender: u32,
    _index: usize,
    _out: *mut RawBorrowInfo,
) {
    arch::naked_asm!("
        # Preserve output buffer pointer in callee-save register, ensuring
        # it is saved on the stack, which is kept 16-byte aligned.
        addi sp, sp, -16
        sw s2, 0(sp)
        mv s2, a2

        # Load the constant syscall number.
        li a7, {sysnum}

        # To the kernel!
        ecall

        # Write all the results out into the raw output buffer.
        sw a0, 0*4(s2)
        sw a1, 1*4(s2)
        sw a2, 2*4(s2)

        # Restore callee-save register and stack pointer and return.
        lw s2, 0(sp)
        addi sp, sp, 16
        ret
        ",
        sysnum = const Sysnum::BorrowInfo as u32,
    )
}

/// Core implementation of the IRQ_CONTROL syscall.
///
/// See the note on syscall stubs at the top of the lib module for rationale.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn sys_irq_control_stub(
    _mask: u32,
    _enable: u32,
) {
    arch::naked_asm!("
        # Load the constant syscall number.
        li a7, {sysnum}

        # To the kernel!
        ecall

        # No results, we can just return now.
        ret
        ",
        sysnum = const Sysnum::IrqControl as u32,
    )
}

/// Core implementation of the PANIC syscall.
///
/// See the note on syscall stubs at the top of the lib module for rationale.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn sys_panic_stub(
    _msg: *const u8,
    _len: usize,
) -> ! {
    arch::naked_asm!("
        # Load the constant syscall number.
        li a7, {sysnum}

        # To the kernel!
        ecall

        # This really shouldn't return. Ensure this:
        unimp
        ",
        sysnum = const Sysnum::Panic as u32,
    )
}

/// Core implementation of the GET_TIMER syscall.
///
/// See the note on syscall stubs at the top of the lib module for rationale.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn sys_get_timer_stub(_out: *mut RawTimerState) {
    arch::naked_asm!("
        # Preserve output buffer pointer in callee-save register, ensuring
        # it is saved on the stack, which is kept 16-byte aligned.
        addi sp, sp, -16
        sw s2, 0(sp)
        mv s2, a0

        # Load the constant syscall number.
        li a7, {sysnum}

        # To the kernel!
        ecall

        # Write all the results out into the raw output buffer.
        sw a0, 0*4(s2)
        sw a1, 1*4(s2)
        sw a2, 2*4(s2)
        sw a3, 3*4(s2)
        sw a4, 4*4(s2)
        sw a5, 5*4(s2)

        # Restore callee-save register and stack pointer and return.
        lw s2, 0(sp)
        addi sp, sp, 16
        ret
        ",
        sysnum = const Sysnum::GetTimer as u32,
    )
}

/// Core implementation of the REFRESH_TASK_ID syscall.
///
/// See the note on syscall stubs at the top of the lib module for rationale.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn sys_refresh_task_id_stub(_tid: u32) -> u32 {
    arch::naked_asm!("
        # Load the constant syscall number.
        li a7, {sysnum}

        # To the kernel!
        ecall

        # Results are placed into the correct registers by the kernel, we can
        # just return now.
        ret
        ",
        sysnum = const Sysnum::RefreshTaskId as u32,
    )
}

/// Core implementation of the POST syscall.
///
/// See the note on syscall stubs at the top of the lib module for rationale.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn sys_post_stub(_tid: u32, _mask: u32) -> u32 {
    arch::naked_asm!("
        # Load the constant syscall number.
        li a7, {sysnum}

        # To the kernel!
        ecall

        # Results are placed into the correct registers by the kernel, we can
        # just return now.
        ret
        ",
        sysnum = const Sysnum::Post as u32,
    )
}

/// Core implementation of the REPLY_FAULT syscall.
///
/// See the note on syscall stubs at the top of the lib module for rationale.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn sys_reply_fault_stub(
    _tid: u32,
    _reason: u32,
) {
    arch::naked_asm!("
        # Load the constant syscall number.
        li a7, {sysnum}

        # To the kernel!
        ecall

        # No results, we can just return now.
        ret
        ",
        sysnum = const Sysnum::ReplyFault as u32,
    )
}

/// Core implementation of the IRQ_STATUS syscall.
///
/// See the note on syscall stubs at the top of the lib module for rationale.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn sys_irq_status_stub(_mask: u32) -> u32 {
    arch::naked_asm!("
        # Load the constant syscall number.
        li a7, {sysnum}

        # To the kernel!
        ecall

        # Results are placed into the correct registers by the kernel, we can
        # just return now.
        ret
        ",
        sysnum = const Sysnum::IrqStatus as u32,
    )
}
