# GDB init script for QEMU RISC-V virt target.
#
# final.elf is stripped; kernel symbols live in the dist output.
# Load with:
#   riscv32-elf-gdb \
#     -ex "file target/qemu-rv32-virt/dist/default/kernel" \
#     -x chips/qemu-rv32-virt/openocd.gdb

target extended-remote :1234

# print demangled symbols
set print asm-demangle on

# set backtrace limit to not have infinite backtrace loops
set backtrace limit 32

# detect kernel faults
break kern::arch::riscv32::handle_fault
