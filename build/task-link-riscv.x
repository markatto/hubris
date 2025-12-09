INCLUDE memory.x

ENTRY(_start);

SECTIONS
{
  PROVIDE(_stack_start = ORIGIN(STACK) + LENGTH(STACK));

  /* ### .text */
  .text : {
    _stext = .;
    *(.text.start*); /* try and pull start symbol to beginning */
    *(.text .text.*);
    . = ALIGN(4);
    __etext = .;
  } > FLASH =0xdededede

  /* ### .rodata */
  .rodata : ALIGN(4)
  {
    *(.srodata .srodata.*);
    *(.rodata .rodata.*);

    /* 4-byte align the end (VMA) of this section.
       This is required by LLD to ensure the LMA of the following .data
       section will have the correct alignment. */
    . = ALIGN(4);
    __erodata = .;
  } > FLASH

  /*
   * Sections in RAM
   *
   * NOTE: the userlib runtime assumes that these sections
   * are 4-byte aligned and padded to 4-byte boundaries.
   */
  .data : ALIGN(4) {
    . = ALIGN(4);
    __sidata = LOADADDR(.data);
    __sdata = .;
    /* Must be called __global_pointer$ for linker relaxations to work. */
    PROVIDE(__global_pointer$ = . + 0x800);
    *(.sdata .sdata.* .sdata2 .sdata2.*);
    *(.data .data.*);
    . = ALIGN(4); /* 4-byte align the end (VMA) of this section */
    __edata = .;
  } > RAM AT>FLASH

  .bss (NOLOAD) : ALIGN(4)
  {
    . = ALIGN(4);
    __sbss = .;
    *(.sbss .sbss.* .bss .bss.*);
    . = ALIGN(4); /* 4-byte align the end (VMA) of this section */
    __ebss = .;
  } > RAM

  .uninit (NOLOAD) : ALIGN(4)
  {
    . = ALIGN(4);
    *(.uninit .uninit.*);
    . = ALIGN(4);
    /* Place the heap right after `.uninit` */
    __sheap = .;
  } > RAM

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

  /* fake output .got section */
  /* Dynamic relocations are unsupported. This section is only used to detect
     relocatable code in the input files and raise an error if relocatable code
     is found */
  .got (INFO) :
  {
    KEEP(*(.got .got.*));
  }

  .eh_frame (INFO) : { KEEP(*(.eh_frame)) }
  .eh_frame_hdr (INFO) : { *(.eh_frame_hdr) }

  /* ## Discarded sections */
  /DISCARD/ :
  {
    *(.riscv.attributes);
  }
}
