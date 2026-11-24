//! OpSy's dialect: the events of the OpSy adapter
//! (target/opsy/include/kymo/opsy/hooks.hpp) as the hooks `meaning` makes
//! spans of. The adapter's events are known by their names,
//! `kymo::opsy::<hook>`, and their fields by name: this module changes with
//! the adapter.
//!
//! The kernel's SysTick and service call handlers, when the kernel family is
//! recorded, are the kernel's handlers `systick` and `service_call`. A wake
//! by `timed_out` or `sleep_elapsed` is by a delay that ran out.

use crate::events::Catalogue;
use crate::meaning::{Dialect, Field, Hook, Precedence};
use crate::target::Result;
use std::collections::HashMap;

pub(crate) const PREFIX: &str = "kymo::opsy::";
/// OpSy's wake reasons for a delay that ran out, given in SysTick.
const ELAPSED: [&str; 2] = ["timed_out", "sleep_elapsed"];

/// The hooks among the catalogue's events, by type id. An adapter event
/// without a field the host reads is an error: host and adapter disagree.
pub(crate) fn dialect(catalogue: &Catalogue) -> Result<Dialect> {
    let mut hooks = HashMap::new();
    for (&id, event) in &catalogue.events {
        let Some(hook) = event.name.strip_prefix(PREFIX) else {
            continue;
        };
        let field = |wanted: &str| Field::of(event, wanted);
        let hook = match hook {
            "task_added" => Hook::TaskAdded {
                task: field("task")?,
                priority: field("priority")?,
                name: None,
            },
            "task_priority_changed" => Hook::TaskPriorityChanged {
                task: field("task")?,
                priority: field("priority")?,
            },
            "task_terminated" => Hook::TaskTerminated { task: field("task")? },
            "task_started" => Hook::TaskStarted { task: field("task")? },
            "enter_idle" => Hook::EnterIdle,
            "task_ready" => Hook::TaskReady {
                task: field("task")?,
                reason: Some(field("reason")?),
            },
            "task_sleep" | "task_wait" | "task_wait_timeout" => Hook::TaskBlocked {
                task: Some(field("task")?),
                wait: None,
            },
            "enter_systick" => Hook::KernelEnter { handler: "systick" },
            "exit_systick" => Hook::KernelExit { handler: "systick" },
            "enter_service_call" => Hook::KernelEnter {
                handler: "service_call",
            },
            "exit_service_call" => Hook::KernelExit {
                handler: "service_call",
            },
            "isr_enter" => Hook::IsrEnter {
                handler: field("handler")?,
                exception: field("exception")?,
            },
            "isr_exit" => Hook::IsrExit {
                handler: field("handler")?,
            },
            "mutex_taken" => Hook::MutexTaken {
                mutex: field("mutex")?,
                task: Some(field("owner")?),
            },
            "mutex_released" => Hook::MutexReleased {
                mutex: field("mutex")?,
                task: Some(field("owner")?),
            },
            "mutex_blocked" => Hook::MutexBlocked {
                mutex: field("mutex")?,
                task: Some(field("waiter")?),
            },
            _ => continue,
        };
        hooks.insert(id, hook);
    }
    Ok(Dialect {
        hooks,
        elapsed: &ELAPSED,
        tick: None,
        requeue: false,
        precedence: Precedence::LowerIsHigher,
    })
}

#[cfg(test)]
mod tests;
