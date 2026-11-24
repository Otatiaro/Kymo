/*
 * QEMU mps2-an386 (Cortex-M4): code in ZBT SRAM at 0x00000000, data in the
 * SRAM at 0x20000000.
 */

MEMORY
{
  CODE (rx)  : ORIGIN = 0x00000000, LENGTH = 4M
  RAM  (rwx) : ORIGIN = 0x20000000, LENGTH = 4M
}

INCLUDE sections.ld
