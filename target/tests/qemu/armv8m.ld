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

INCLUDE sections.ld
