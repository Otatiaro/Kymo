/*
 * QEMU mps2-an505 (Cortex-M33). The core comes out of reset in the Secure
 * state, so code and data both live in the Secure ZBT SRAM at 0x10000000;
 * the SRAM at 0x30000000 sits behind a protection controller that blocks it
 * at reset.
 */

MEMORY
{
  CODE (rx)  : ORIGIN = 0x10000000, LENGTH = 2M
  RAM  (rwx) : ORIGIN = 0x10200000, LENGTH = 2M
}

ENTRY(Reset_Handler)

SECTIONS
{
  .isr_vector : { KEEP(*(.isr_vector)) } > CODE
  .text : { *(.text*) *(.rodata*) . = ALIGN(4); } > CODE

  .data : { . = ALIGN(4); _sdata = .; *(.data*) . = ALIGN(4); _edata = .; } > RAM AT> CODE
  _sidata = LOADADDR(.data);

  .bss (NOLOAD) : { . = ALIGN(4); _sbss = .; *(.bss*) *(COMMON) . = ALIGN(4); _ebss = .; } > RAM

  _estack = ORIGIN(RAM) + LENGTH(RAM);

  /DISCARD/ : { *(.ARM.exidx*) *(.ARM.extab*) }
}
