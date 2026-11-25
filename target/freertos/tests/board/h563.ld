/*
 * NUCLEO-H563ZI: STM32H563ZI, 2 MB of flash, 640 KB of SRAM (SRAM1-3
 * contiguous), in the regions of the QEMU images' sections.
 */

MEMORY
{
  CODE (rx)  : ORIGIN = 0x08000000, LENGTH = 2M
  RAM  (rwx) : ORIGIN = 0x20000000, LENGTH = 640K
}

INCLUDE sections.ld
