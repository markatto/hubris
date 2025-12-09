# RISC-V Port Status

## Current State

The RISC-V port is partially complete. Tasks compile and link successfully,
but the kernel needs updates to compile with current upstream Hubris.

## What Works

- **userlib syscall stubs** (`sys/userlib/src/arch/riscv32.rs`)
  - All 15 syscall stubs implemented with modern syntax
  - Uses `#[unsafe(naked)]` and `arch::naked_asm!`

- **Task linking**
  - RISC-V linker script (`build/task-link-riscv.x`)
  - xtask MPU alignment config for RISC-V PMP

- **Minimal supervisor** (`task/jefe-stub`)
  - Simple fault-handler that restarts crashed tasks
  - No IPC server, no dump support

- **Idle task** - Already had RISC-V support

## What Needs Work

### Kernel (`sys/kern/src/arch/riscv32.rs`)

The kernel arch file is from jperkin's 2022 port and needs updates:

1. **Missing `RegionDescExt` struct and `compute_region_extension_data` function**
   - Current upstream pre-computes MPU/PMP register values at build time
   - ARM version has this in `arm_m.rs` lines 360-565
   - RISC-V needs equivalent for PMP (Physical Memory Protection)

2. **Rust syntax updates**
   - `#[naked]` → `#[unsafe(naked)]`
   - Add `use core::arch::asm;`
   - Rust 2024 requires explicit `unsafe {}` blocks inside unsafe functions

3. **Missing dependency**
   - `riscv_semihosting` crate not in Cargo.toml (used for `klog!` macro)

4. **Timer is hardcoded for FE310 (HiFive/RED-V)**
   - `MTIME` = 0x0200_BFF8
   - `MTIMECMP` = 0x0200_4000
   - RP2350 uses different timer peripheral (TIMER0 at 0x400b0000)

5. **Missing `crate::app` module reference**
   - Old code referenced `crate::app::RegionAttributes`
   - Current upstream uses `abi::RegionAttributes` or similar

### Origin of the Code

Jonathan Perkin wrote the RISC-V kernel support from scratch in January 2022
(commit 940113c). It was not a mechanical transformation of the ARM code -
the structure differs significantly due to different interrupt handling:

- ARM uses NVIC, PendSV for deferred context switches
- RISC-V uses trap handlers with direct save/restore

The port targeted the SiFive FE310 core (HiFive RevB, SparkFun RED-V boards).

## Files Modified/Added

```
sys/userlib/src/arch/mod.rs      - Architecture selector
sys/userlib/src/arch/arm_m.rs    - ARM stubs (refactored from lib.rs)
sys/userlib/src/arch/riscv32.rs  - RISC-V stubs
sys/userlib/src/lib.rs           - Uses arch module

sys/kern/src/arch/riscv32.rs     - Kernel arch (needs updates)
sys/kern/build.rs                - Added riscv32 target

build/task-link-riscv.x          - RISC-V linker script
build/xtask/src/config.rs        - MPU alignment for RISC-V
build/xtask/src/dist.rs          - Arch enum (already had RISC-V)

task/jefe-stub/                  - Minimal supervisor for bringup

app/pico2/app.toml               - Uses jefe-stub
chips/rp2350/                    - Chip definition
boards/pico2/                    - Board definition
```

## Next Steps

1. Add `RegionDescExt` and `compute_region_extension_data` to riscv32.rs
2. Fix Rust syntax for 2024 edition
3. Add riscv_semihosting to kern/Cargo.toml (or remove klog dependency)
4. Implement RP2350 timer handling (or stub it out initially)
5. Test build and iterate on remaining issues
