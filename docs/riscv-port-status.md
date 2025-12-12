# RISC-V Port Status

## What Works

The RISC-V port boots and runs on QEMU's rv32 virt platform.

**Build:** `cargo xtask dist app/qemu-rv32-virt/app.toml`

**Run:**
```
qemu-system-riscv32 -machine virt -nographic -bios none \
    -semihosting-config enable=on,target=native \
    -kernel target/qemu-rv32-virt/dist/default/final.elf -gdb tcp::2331
```

### Kernel
- Context switching, syscalls, PMP memory protection
- CLINT timer interrupts, PLIC external interrupts
- Fault detection with CSR diagnostics

### Tasks
- jefe (full supervisor), idle, drv-ns16550-uart, task-hello

### Humility (in `../humility/`)
- `humility-arch` crate with architecture-agnostic Register trait
- `humility-arch-riscv` crate with RISC-V register definitions
- GDB probe supports QEMU as server type
- Architecture detection (ARM vs RISC-V) in tasks command

## What Needs Work

- `humility tasks` has ARM-specific SavedState references
- Timer/irq support for actual hardware (all currently available hardware uses custom implementations; plan on supporting at least rp2350 and esp32-c3.
