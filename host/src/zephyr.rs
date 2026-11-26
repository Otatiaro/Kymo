//! Zephyr's dialect: the events of the Zephyr adapter
//! (target/zephyr/include/kymo/zephyr/events.hpp) as the hooks `meaning`
//! makes spans of. The adapter's events are known by their names,
//! `kymo::zephyr::<hook>`, and their fields by name: this module changes
//! with the adapter.
//!
//! - A thread's priority is higher the lower its value, and may be negative
//!   (a cooperative thread): it reads as it is. sched_priority_set gives the
//!   priority itself, set by the application or by a mutex's inheritance; a
//!   ready thread whose priority changes stays in the ready queue, with no
//!   sched_ready.
//! - sched_ready gives no reason: a wake's reason is the wait the thread
//!   leaves, the block event's name without its `_blocking` suffix: `sleep`,
//!   `sem_take`, `msgq_get`, `msgq_put`, `event_wait`, `pipe_read`,
//!   `pipe_write`, `mutex`.
//! - A sleep of 0 ticks is a yield, no block.
//! - A block is the thread's only once the core goes to another thread: a
//!   k_msgq_get may report blocking and then be handed a message without
//!   switching.
//! - A wake emitted in SysTick, exception 15, is by a timeout that ran out.
//! - The kernel's start gives the idle thread; a thread may be named after
//!   it is created.

use crate::events::Catalogue;
use crate::meaning::{Dialect, Field, Hook, Precedence};
use crate::target::Result;
use std::collections::HashMap;

pub(crate) const PREFIX: &str = "kymo::zephyr::";
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
            timeout: None,
        };
        let hook = match hook {
            "starting" => Hook::Starting { idle: field("idle")? },
            "thread_create" => Hook::TaskAdded {
                task: field("thread")?,
                priority: field("priority")?,
                name: Some(Field::text_of(event, "name")?),
            },
            "thread_name_set" => Hook::TaskNamed {
                task: field("thread")?,
                name: Field::text_of(event, "name")?,
            },
            "thread_sched_abort" => Hook::TaskTerminated { task: field("thread")? },
            "thread_switched_in" => Hook::TaskStarted { task: field("thread")? },
            "thread_sched_priority_set" => Hook::TaskPriorityChanged {
                task: field("thread")?,
                priority: field("priority")?,
            },
            "thread_sched_ready" => Hook::TaskReady {
                task: field("thread")?,
                reason: None,
            },
            "thread_sleep_ticks_enter" => Hook::TaskBlocked {
                task: None,
                wait: Some("sleep"),
                timeout: Some(field("ticks")?),
            },
            "sem_take_blocking" => blocked("sem_take"),
            "msgq_get_blocking" => blocked("msgq_get"),
            "msgq_put_blocking" => blocked("msgq_put"),
            "event_wait_blocking" => blocked("event_wait"),
            "pipe_read_blocking" => blocked("pipe_read"),
            "pipe_write_blocking" => blocked("pipe_write"),
            "mutex_lock_blocking" => Hook::MutexBlocked {
                mutex: field("mutex")?,
                task: None,
            },
            "mutex_taken" => Hook::MutexTaken {
                mutex: field("mutex")?,
                task: None,
            },
            "mutex_released" => Hook::MutexReleased {
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
        requeue: false,
        confirm_blocks: true,
        precedence: Precedence::LowerIsHigher,
    })
}

#[cfg(test)]
mod tests;
