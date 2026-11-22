/*
 * Sections of the bench image. Each board script defines FLASH and RAM, then
 * includes this file.
 */

ENTRY(Reset_Handler)

SECTIONS
{
  .isr_vector : { KEEP(*(.isr_vector)) } > FLASH

  /* Placed explicitly: as an orphan section the linker would put it ahead of
     the vector table. The host matches it against the ELF it decodes with. */
  .note.gnu.build-id : { KEEP(*(.note.gnu.build-id)) } > FLASH

  .text :
  {
    *(.text*)
    *(.rodata*)
    . = ALIGN(4);
  } > FLASH

  .data :
  {
    . = ALIGN(4);
    _sdata = .;
    *(.data*)
    . = ALIGN(4);
    _edata = .;
  } > RAM AT> FLASH

  _sidata = LOADADDR(.data);

  .bss (NOLOAD) :
  {
    . = ALIGN(4);
    _sbss = .;
    *(.bss*)
    *(COMMON)
    . = ALIGN(4);
    _ebss = .;
  } > RAM

  _estack = ORIGIN(RAM) + LENGTH(RAM);

  /DISCARD/ : { *(.ARM.exidx*) *(.ARM.extab*) *(.init_array*) *(.fini_array*) }
}
