//! The process out of Windows power throttling. Windows throttles a process
//! without a window in the foreground: it slows its execution, by several
//! times after a few seconds of work, and coarsens its timers. `kymo record`
//! and `kymo mcp` run so, the one draining a probe, the other answering an
//! agent that waits on each call.

/// Takes the process out of power throttling; nothing to do off Windows.
#[cfg(windows)]
pub fn unthrottle() -> std::io::Result<()> {
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE, ProcessPowerThrottling,
        SetProcessInformation,
    };
    // Every mechanism named in the control mask, none in the state mask: off.
    let unthrottled = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
        StateMask: 0,
    };
    // SAFETY: the pseudo handle of the current process is always valid; the
    // state passed is the structure ProcessPowerThrottling takes, with its
    // size.
    let done = unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            (&raw const unthrottled).cast(),
            size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
    };
    if done == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn unthrottle() -> std::io::Result<()> {
    Ok(())
}
