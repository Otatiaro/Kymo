//! FreeRTOS's dialect: the events of the FreeRTOS adapter
//! (target/freertos/include/kymo/freertos/events.hpp) as the hooks `meaning`
//! makes spans of. The adapter's events are known by their names,
//! `kymo::freertos::<macro>`, and their fields by name: this module changes
//! with the adapter.
//!
//! - A task's priority is higher the higher its value. vTaskPrioritySet gives
//!   the base priority, inheritance the priority itself; a ready task whose
//!   priority changes is moved within the ready list, with a
//!   moved_task_to_ready_state that is no wake.
//! - moved_task_to_ready_state gives no reason: a wake's reason is the wait
//!   the task leaves, the block event's name without its `blocking_on_` or
//!   `task_` prefix and `_block` suffix: `delay`, `delay_until`,
//!   `queue_receive`, `queue_send`, `queue_peek`, `notify_take`,
//!   `notify_wait`, `event_group_sync`, `event_group_wait_bits`,
//!   `stream_buffer_send`, `stream_buffer_receive`, `mutex`, `suspend`.
//! - A wake emitted in SysTick, exception 15, is by a delay that ran out.
//! - The scheduler's start gives the idle task.
//! - task_resume, queue_send and task_notify make no span: the wake that
//!   follows them gives their context.

use crate::events::Catalogue;
use crate::meaning::{Dialect, Field, Hook, Precedence};
use crate::target::Result;
use std::collections::HashMap;

pub(crate) const PREFIX: &str = "kymo::freertos::";
/// SysTick's exception number.
const SYSTICK: u16 = 15;

/// The hooks among the catalogue's events, by type id. An adapter event
/// without a field the host reads is an error: host and adapter disagree.
pub(crate) fn dialect(catalogue: &Catalogue) -> Result<Dialect> {
    let mut hooks = HashMap::new();
    for (&id, event) in &catalogue.events {
        let Some(hook) = event.name.strip_prefix(PREFIX) else {
            continue;
        };
        let field = |wanted: &str| Field::of(event, wanted);
        let blocked = |wait| Hook::TaskBlocked {
            task: None,
            wait: Some(wait),
        };
        let hook = match hook {
            "starting" => Hook::Starting { idle: field("idle")? },
            "task_create" => Hook::TaskAdded {
                task: field("task")?,
                priority: field("priority")?,
                name: Some(Field::text_of(event, "name")?),
            },
            "task_delete" => Hook::TaskTerminated { task: field("task")? },
            "task_switched_in" => Hook::TaskStarted { task: field("task")? },
            "task_priority_set" => Hook::TaskBasePriority {
                task: field("task")?,
                priority: field("priority")?,
            },
            "task_priority_inherit" | "task_priority_disinherit" => Hook::TaskPriorityChanged {
                task: field("task")?,
                priority: field("priority")?,
            },
            "moved_task_to_ready_state" => Hook::TaskReady {
                task: field("task")?,
                reason: None,
            },
            "task_suspend" => Hook::TaskSuspended { task: field("task")? },
            "task_delay" => blocked("delay"),
            "task_delay_until" => blocked("delay_until"),
            "blocking_on_queue_receive" => blocked("queue_receive"),
            "blocking_on_queue_send" => blocked("queue_send"),
            "blocking_on_queue_peek" => blocked("queue_peek"),
            "task_notify_take_block" => blocked("notify_take"),
            "task_notify_wait_block" => blocked("notify_wait"),
            "event_group_sync_block" => blocked("event_group_sync"),
            "event_group_wait_bits_block" => blocked("event_group_wait_bits"),
            "blocking_on_stream_buffer_send" => blocked("stream_buffer_send"),
            "blocking_on_stream_buffer_receive" => blocked("stream_buffer_receive"),
            "mutex_taken" => Hook::MutexTaken {
                mutex: field("mutex")?,
                task: None,
            },
            "mutex_released" => Hook::MutexReleased {
                mutex: field("mutex")?,
                task: None,
            },
            "mutex_blocked" => Hook::MutexBlocked {
                mutex: field("mutex")?,
                task: None,
            },
            "isr_enter" => Hook::IsrEnter {
                handler: field("handler")?,
                exception: field("exception")?,
            },
            "isr_exit" => Hook::IsrExitException {
                exception: field("exception")?,
            },
            _ => continue,
        };
        hooks.insert(id, hook);
    }
    Ok(Dialect {
        hooks,
        elapsed: &[],
        tick: Some(SYSTICK),
        requeue: true,
        precedence: Precedence::HigherIsHigher,
    })
}

#[cfg(test)]
mod tests;
