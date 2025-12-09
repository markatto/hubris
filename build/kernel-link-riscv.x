/* RISC-V kernel linker script additions for Hubris
 *
 * This content is appended to memory.x by the build system.
 * The kernel uses riscv-rt for startup and trap handling. This provides
 * the REGION_ALIAS mappings that riscv-rt expects, plus Hubris-specific
 * metadata sections.
 */

/* Map our memory regions to riscv-rt's expected region aliases */
REGION_ALIAS("REGION_TEXT", FLASH);
REGION_ALIAS("REGION_RODATA", FLASH);
REGION_ALIAS("REGION_DATA", RAM);
REGION_ALIAS("REGION_BSS", RAM);
REGION_ALIAS("REGION_HEAP", RAM);
REGION_ALIAS("REGION_STACK", STACK);

/* Symbols for Hubris kernel */
PROVIDE(_stack_start = ORIGIN(STACK) + LENGTH(STACK));

SECTIONS
{
  /* ## .task_slot_table */
  /* Table of TaskSlot instances and their names. Used to resolve task
     dependencies during packaging. */
  .task_slot_table (INFO) : {
    . = .;
    KEEP(*(.task_slot_table));
  }

  /* ## .caboose_pos_table */
  /* Table of CaboosePos instances and their names. Used to record caboose
     position during packaging. */
  .caboose_pos_table (INFO) : {
    . = .;
    KEEP(*(.caboose_pos_table));
  }

  /* ## .idolatry */
  .idolatry (INFO) : {
    . = .;
    KEEP(*(.idolatry));
  }

  /* ## Discarded sections */
  /DISCARD/ :
  {
    *(.riscv.attributes);
    *(.eh_frame);
    *(.eh_frame_hdr);
  }
}
