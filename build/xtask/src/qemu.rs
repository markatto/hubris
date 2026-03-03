// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::process::Command;

use anyhow::{bail, Result};

use crate::config::Config;
use crate::dist::Arch;

/// Determine the QEMU binary name from the target triple.
fn qemu_binary(target: &str) -> Result<&'static str> {
    if target.starts_with("riscv32") {
        Ok("qemu-system-riscv32")
    } else if target.starts_with("riscv64") {
        Ok("qemu-system-riscv64")
    } else {
        bail!(
            "no QEMU binary known for target '{}'; \
             `cargo xtask qemu` only supports emulator targets",
            target
        );
    }
}

/// Determine the QEMU machine type from the board name.
fn qemu_machine(board: &str) -> Result<&'static str> {
    match board {
        "qemu-rv32-virt" => Ok("virt"),
        _ => bail!(
            "board '{}' is not a known QEMU target; \
             `cargo xtask qemu` only supports emulator targets",
            board
        ),
    }
}

/// Launch QEMU for an emulator-based target.
///
/// The image must already be built (i.e. `dist::package` should have
/// been called). `image_name` selects which image to boot (usually
/// "default"). If `gdb` is true, QEMU starts paused with a GDB
/// server on port 1234.
pub fn run(cfg: &Config, image_name: &str, gdb: bool) -> Result<()> {
    let arch = Arch::from_target(&cfg.target)?;
    if arch != Arch::RiscV {
        bail!(
            "`cargo xtask qemu` only supports RISC-V emulator targets \
             (got target '{}')",
            cfg.target
        );
    }

    let qemu_bin = qemu_binary(&cfg.target)?;
    let machine = qemu_machine(&cfg.board)?;

    let elf_path = format!("target/{}/dist/{}/final.elf", cfg.name, image_name);

    let mut cmd = Command::new(qemu_bin);
    cmd.args(["-machine", machine])
        .args(["-nographic"])
        .args(["-bios", "none"])
        .args(["-kernel", &elf_path]);

    // Enable semihosting if the kernel uses it for logging.
    if cfg
        .kernel
        .features
        .contains(&"klog-semihosting".to_string())
    {
        cmd.args(["-semihosting-config", "enable=on,target=native"]);
    }

    if gdb {
        cmd.args(["-gdb", "tcp::1234", "-S"]);
        eprintln!("QEMU waiting for GDB on :1234. Connect with:");
        eprintln!(
            "  riscv32-elf-gdb \
             -ex \"file target/{}/dist/{}/kernel\" \
             -x chips/{}/openocd.gdb",
            cfg.name, image_name, cfg.board
        );
    }

    eprintln!("Launching: {qemu_bin} (Ctrl-A X to quit)");

    let status = cmd.status()?;

    if !status.success() {
        bail!("QEMU exited with {}", status);
    }

    Ok(())
}
