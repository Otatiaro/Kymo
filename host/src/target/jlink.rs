//! Target access through SEGGER's J-Link library (`JLink_x64.dll`,
//! `libjlinkarm.so`), which drives a J-Link over its own USB driver.
//!
//! The library keeps one connection per process, so a single J-Link target
//! can be open at a time.

use super::{Connection, Result, Target};
use libloading::Library;
use std::ffi::{CStr, CString, c_char, c_int};

#[cfg(windows)]
const DEFAULT_PATHS: &[&str] = &[r"C:\Program Files\SEGGER\JLink\JLink_x64.dll"];
#[cfg(not(windows))]
const DEFAULT_PATHS: &[&str] = &["/opt/SEGGER/JLink/libjlinkarm.so", "libjlinkarm.so"];

/// Overrides the library path.
const PATH_VARIABLE: &str = "KYMO_JLINK_LIBRARY";

const INTERFACE_SWD: c_int = 1;

type LogCallback = unsafe extern "C" fn(*const c_char);

/// The entry points used, with the signatures of SEGGER's JLinkARMDLL.h.
struct Api {
    select_by_serial: unsafe extern "C" fn(u32) -> c_int,
    open: unsafe extern "C" fn(Option<LogCallback>, Option<LogCallback>) -> *const c_char,
    execute: unsafe extern "C" fn(*const c_char, *mut c_char, c_int) -> c_int,
    select_interface: unsafe extern "C" fn(c_int) -> c_int,
    set_speed: unsafe extern "C" fn(u32),
    connect: unsafe extern "C" fn() -> c_int,
    /// (address, count, words, status per word) -> words read, negative on error
    read_u32: unsafe extern "C" fn(u32, u32, *mut u32, *mut u8) -> c_int,
    /// (address, value) -> 0 on success
    write_u32: unsafe extern "C" fn(u32, u32) -> c_int,
    close: unsafe extern "C" fn(),
}

impl Api {
    /// # Safety
    /// The library must be SEGGER's J-Link library.
    unsafe fn resolve(library: &Library) -> Result<Self> {
        // SAFETY: the caller guarantees the library, hence the signatures.
        unsafe {
            Ok(Self {
                select_by_serial: *library.get(b"JLINKARM_EMU_SelectByUSBSN")?,
                open: *library.get(b"JLINKARM_OpenEx")?,
                execute: *library.get(b"JLINKARM_ExecCommand")?,
                select_interface: *library.get(b"JLINKARM_TIF_Select")?,
                set_speed: *library.get(b"JLINKARM_SetSpeed")?,
                connect: *library.get(b"JLINKARM_Connect")?,
                read_u32: *library.get(b"JLINKARM_ReadMemU32")?,
                write_u32: *library.get(b"JLINKARM_WriteU32")?,
                close: *library.get(b"JLINKARM_Close")?,
            })
        }
    }
}

unsafe extern "C" fn report_error(message: *const c_char) {
    if !message.is_null() {
        // SAFETY: the library passes a NUL-terminated string.
        let text = unsafe { CStr::from_ptr(message) }.to_string_lossy();
        eprintln!("J-Link: {text}");
    }
}

pub struct JLinkTarget {
    api: Api,
    /// Keeps the entry points of `api` loaded; dropped last.
    _library: Library,
    status: Vec<u8>,
}

impl JLinkTarget {
    pub fn open(connection: &Connection) -> Result<Self> {
        let chip = connection.chip.as_deref().ok_or("J-Link needs --chip")?;
        let library = load_library()?;
        // SAFETY: load_library only loads the J-Link library.
        let api = unsafe { Api::resolve(&library)? };
        let target = Self { api, _library: library, status: Vec::new() };
        // SAFETY: the calls follow the order the library documents: select
        // the probe, open, configure, connect.
        unsafe {
            if let Some(serial) = &connection.probe {
                let serial: u32 = serial.parse()?;
                if (target.api.select_by_serial)(serial) < 0 {
                    return Err(format!("no J-Link with serial number {serial}").into());
                }
            }
            let failure = (target.api.open)(None, Some(report_error));
            if !failure.is_null() {
                let text = CStr::from_ptr(failure).to_string_lossy();
                return Err(format!("J-Link open failed: {text}").into());
            }
            // No dialog box: a bench runs unattended.
            target.command("SetBatchMode = 1")?;
            target.command("SuppressInfoUpdateFW")?;
            target.command(&format!("Device = {chip}"))?;
            (target.api.select_interface)(INTERFACE_SWD);
            (target.api.set_speed)(connection.speed_khz);
            if (target.api.connect)() < 0 {
                return Err("J-Link could not connect to the target".into());
            }
        }
        Ok(target)
    }

    fn command(&self, command: &str) -> Result<()> {
        let text = CString::new(command)?;
        let mut error = [0 as c_char; 256];
        // SAFETY: both buffers are valid for the call; the size is the buffer's.
        unsafe { (self.api.execute)(text.as_ptr(), error.as_mut_ptr(), error.len() as c_int) };
        if error[0] != 0 {
            // SAFETY: the library NUL-terminates what it writes in the buffer.
            let message = unsafe { CStr::from_ptr(error.as_ptr()) }.to_string_lossy();
            return Err(format!("J-Link command {command:?}: {message}").into());
        }
        Ok(())
    }
}

fn load_library() -> Result<Library> {
    let candidates: Vec<String> = match std::env::var(PATH_VARIABLE) {
        Ok(path) => vec![path],
        Err(_) => DEFAULT_PATHS.iter().map(|path| path.to_string()).collect(),
    };
    let mut errors = Vec::new();
    for path in &candidates {
        // SAFETY: loading the vendor library runs its initialisation code,
        // as the vendor tools do.
        match unsafe { Library::new(path) } {
            Ok(library) => return Ok(library),
            Err(error) => errors.push(format!("{path}: {error}")),
        }
    }
    Err(format!("J-Link library not found ({}); set {PATH_VARIABLE}", errors.join("; ")).into())
}

impl Target for JLinkTarget {
    fn read_words(&mut self, address: u32, words: &mut [u32]) -> Result<()> {
        self.status.resize(words.len(), 0);
        // SAFETY: both buffers hold words.len() items.
        let read = unsafe {
            (self.api.read_u32)(address, words.len() as u32, words.as_mut_ptr(), self.status.as_mut_ptr())
        };
        if read != words.len() as c_int {
            return Err(format!("J-Link read at {address:#010x} returned {read}").into());
        }
        Ok(())
    }

    fn write_word(&mut self, address: u32, value: u32) -> Result<()> {
        // SAFETY: plain values.
        let result = unsafe { (self.api.write_u32)(address, value) };
        if result != 0 {
            return Err(format!("J-Link write at {address:#010x} returned {result}").into());
        }
        Ok(())
    }
}

impl Drop for JLinkTarget {
    fn drop(&mut self) {
        // SAFETY: ends the connection opened in `open`; the library is still loaded.
        unsafe { (self.api.close)() };
    }
}
