/*
 * The event type ids (kymo/type_id.hpp): a section not loaded on the target,
 * at address 0, where the offset of each type's byte is its id. Zephyr links
 * with orphan sections as errors, so the module places it.
 */
.kymo.ids 0 : { *(.kymo.ids) }
