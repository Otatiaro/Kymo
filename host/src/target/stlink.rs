//! Target access to ST-Link probes through ST's STM32CubeProgrammer library
//! (`CubeProgrammer_API.dll`, `libCubeProgrammer_API.so`).
//!
//! The library keeps one connection per process, so a single ST-Link target
//! can be open at a time.

use super::{Connection, Result, Target};
use libloading::Library;
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::path::{Path, PathBuf};

#[cfg(windows)]
const DEFAULT_PATHS: &[&str] =
    &[r"C:\Program Files\STMicroelectronics\STM32Cube\STM32CubeProgrammer\api\lib\CubeProgrammer_API.dll"];
#[cfg(not(windows))]
const DEFAULT_PATHS: &[&str] = &[
    "/usr/local/STMicroelectronics/STM32Cube/STM32CubeProgrammer/lib/libCubeProgrammer_API.so",
    "libCubeProgrammer_API.so",
];

/// Overrides the library path.
const PATH_VARIABLE: &str = "KYMO_STLINK_LIBRARY";

const PORT_SWD: c_int = 1;
const MODE_HOTPLUG: c_int = 1;

/// `debugConnectParameters` from CubeProgrammer_API.h.
#[repr(C)]
#[derive(Clone, Copy)]
struct ConnectParameters {
    debug_port: c_int,
    index: c_int,
    serial_number: [c_char; 33],
    firmware_version: [c_char; 20],
    target_voltage: [c_char; 5],
    access_port_number: c_int,
    access_port: c_int,
    connection_mode: c_int,
    reset_mode: c_int,
    is_old_firmware: c_int,
    jtag_frequencies: [c_uint; 12],
    jtag_frequency_count: c_uint,
    swd_frequencies: [c_uint; 12],
    swd_frequency_count: c_uint,
    frequency: c_int,
    is_bridge: c_int,
    shared: c_int,
    board: [c_char; 100],
    debug_in_sleep: c_int,
    speed: c_int,
}
const _: () = assert!(std::mem::size_of::<ConnectParameters>() == 312);

/// `displayCallBacks` from CubeProgrammer_API.h.
#[repr(C)]
struct DisplayCallbacks {
    init_progress_bar: unsafe extern "C" fn(),
    log_message: unsafe extern "C" fn(c_int, *const u16),
    load_bar: unsafe extern "C" fn(c_int, c_int),
}

unsafe extern "C" fn ignore_progress() {}
/// Message types from DisplayManager.h (MSGTYPE).
const MESSAGE_ERROR: c_int = 5;
const MESSAGE_WARNING: c_int = 4;

/// Passes the library's errors and warnings on, drops the rest.
unsafe extern "C" fn report_message(kind: c_int, text: *const u16) {
    if text.is_null()
        || std::env::var_os("KYMO_STLINK_LOG").is_none() && kind != MESSAGE_ERROR && kind != MESSAGE_WARNING
    {
        return;
    }
    // SAFETY: the library passes a NUL-terminated wide string.
    let units = unsafe {
        let length = (0..).take_while(|&i| *text.add(i) != 0).count();
        std::slice::from_raw_parts(text, length)
    };
    eprintln!(
        "CubeProgrammer [{kind}]: {}",
        String::from_utf16_lossy(units).trim_end()
    );
}
unsafe extern "C" fn ignore_load(_: c_int, _: c_int) {}

/// The entry points used, with the signatures of CubeProgrammer_API.h.
struct Api {
    set_display_callbacks: unsafe extern "C" fn(DisplayCallbacks),
    list: unsafe extern "C" fn(*mut *mut ConnectParameters, c_int) -> c_int,
    delete_list: unsafe extern "C" fn(),
    connect: unsafe extern "C" fn(ConnectParameters) -> c_int,
    /// (address, out buffer allocated by the library, size) -> 0 on success
    read: unsafe extern "C" fn(c_uint, *mut *mut u8, c_uint) -> c_int,
    free: unsafe extern "C" fn(*mut c_void),
    disconnect: unsafe extern "C" fn(),
    set_loaders_path: unsafe extern "C" fn(*const c_char),
}

impl Api {
    /// # Safety
    /// The library must be ST's CubeProgrammer library.
    unsafe fn resolve(library: &Library) -> Result<Self> {
        // SAFETY: the caller guarantees the library, hence the signatures.
        unsafe {
            Ok(Self {
                set_display_callbacks: *library.get(b"setDisplayCallbacks")?,
                list: *library.get(b"getStLinkList")?,
                delete_list: *library.get(b"deleteInterfaceList")?,
                connect: *library.get(b"connectStLink")?,
                read: *library.get(b"readMemory")?,
                free: *library.get(b"freeLibraryMemory")?,
                disconnect: *library.get(b"disconnect")?,
                set_loaders_path: *library.get(b"setLoadersPath")?,
            })
        }
    }
}

pub struct StLinkTarget {
    api: Api,
    /// Keeps the entry points of `api` loaded; dropped last.
    _library: Library,
    speed_khz: u32,
}

impl StLinkTarget {
    pub fn open(connection: &Connection) -> Result<Self> {
        let (library, directory) = load_library()?;
        // The library finds its device database from the loaders path, the
        // directory it sits in; without it, it identifies no device.
        let directory = CString::new(directory.to_string_lossy().into_owned())?;
        // SAFETY: load_library only loads the CubeProgrammer library.
        let api = unsafe { Api::resolve(&library)? };
        let mut target = Self {
            api,
            _library: library,
            speed_khz: connection.speed_khz,
        };
        // SAFETY: the list returned by the library holds `count` entries and
        // stays valid until delete_list; the chosen entry is copied first.
        unsafe {
            (target.api.set_loaders_path)(directory.as_ptr());
            (target.api.set_display_callbacks)(DisplayCallbacks {
                init_progress_bar: ignore_progress,
                log_message: report_message,
                load_bar: ignore_load,
            });
            let mut list: *mut ConnectParameters = std::ptr::null_mut();
            let count = (target.api.list)(&mut list, 0);
            let probes: Vec<ConnectParameters> = if count > 0 && !list.is_null() {
                std::slice::from_raw_parts(list, count as usize).to_vec()
            } else {
                Vec::new()
            };
            let serial_of = |probe: &ConnectParameters| {
                CStr::from_ptr(probe.serial_number.as_ptr())
                    .to_string_lossy()
                    .into_owned()
            };
            let mut parameters = match &connection.probe {
                Some(serial) => *probes
                    .iter()
                    .find(|probe| serial_of(probe).eq_ignore_ascii_case(serial))
                    .ok_or_else(|| {
                        let found: Vec<String> = probes.iter().map(serial_of).collect();
                        format!("no ST-Link with serial number {serial} (found: {})", found.join(", "))
                    })?,
                None => match probes.as_slice() {
                    [only] => *only,
                    [] => return Err("no ST-Link found".into()),
                    _ => return Err("several ST-Links found: pick one with --probe-id".into()),
                },
            };
            parameters.debug_port = PORT_SWD;
            parameters.connection_mode = MODE_HOTPLUG;
            target.speed_khz = supported_frequency(&parameters, connection.speed_khz);
            parameters.frequency = target.speed_khz as c_int;
            parameters.shared = 0;
            let result = (target.api.connect)(parameters);
            (target.api.delete_list)();
            if result != 0 {
                return Err(format!("ST-Link connection failed ({result})").into());
            }
        }
        Ok(target)
    }
}

/// The fastest SWD clock the probe supports at or below `requested_khz`,
/// else its slowest: the probe takes only the values it lists.
fn supported_frequency(probe: &ConnectParameters, requested_khz: u32) -> u32 {
    let count = (probe.swd_frequency_count as usize).min(probe.swd_frequencies.len());
    let listed = &probe.swd_frequencies[..count];
    let below = listed.iter().copied().filter(|&khz| khz <= requested_khz).max();
    below.or_else(|| listed.iter().copied().min()).unwrap_or(requested_khz)
}

/// The library and the directory it was loaded from.
fn load_library() -> Result<(Library, PathBuf)> {
    let candidates: Vec<PathBuf> = match std::env::var_os(PATH_VARIABLE) {
        Some(path) => vec![path.into()],
        None => DEFAULT_PATHS.iter().map(PathBuf::from).collect(),
    };
    let mut errors = Vec::new();
    for path in &candidates {
        match load(path) {
            Ok(library) => {
                let directory = path.parent().map(Path::to_path_buf).unwrap_or_default();
                return Ok((library, directory));
            }
            Err(error) => errors.push(format!("{}: {error}", path.display())),
        }
    }
    Err(format!(
        "CubeProgrammer library not found ({}); set {PATH_VARIABLE}",
        errors.join("; ")
    )
    .into())
}

#[cfg(windows)]
fn load(path: &Path) -> std::result::Result<Library, libloading::Error> {
    use libloading::os::windows::{LOAD_WITH_ALTERED_SEARCH_PATH, Library as WindowsLibrary};
    // The library depends on DLLs that sit next to it: search its directory.
    // SAFETY: loading the vendor library runs its initialisation code, as the
    // vendor tools do.
    unsafe { WindowsLibrary::load_with_flags(path, LOAD_WITH_ALTERED_SEARCH_PATH) }.map(Into::into)
}

#[cfg(not(windows))]
fn load(path: &Path) -> std::result::Result<Library, libloading::Error> {
    // SAFETY: as above.
    unsafe { Library::new(path) }
}

impl Target for StLinkTarget {
    fn speed_khz(&self) -> Option<u32> {
        Some(self.speed_khz)
    }

    fn read_words(&mut self, address: u32, words: &mut [u32]) -> Result<()> {
        let size = (words.len() * 4) as c_uint;
        let mut data: *mut u8 = std::ptr::null_mut();
        // SAFETY: on success the library allocates `size` bytes at `data`,
        // released with its own free.
        unsafe {
            let result = (self.api.read)(address, &mut data, size);
            if result != 0 || data.is_null() {
                return Err(format!("ST-Link read at {address:#010x} failed ({result})").into());
            }
            let bytes = std::slice::from_raw_parts(data, size as usize);
            for (word, chunk) in words.iter_mut().zip(bytes.as_chunks::<4>().0) {
                *word = u32::from_le_bytes(*chunk);
            }
            (self.api.free)(data.cast());
        }
        Ok(())
    }
}

impl Drop for StLinkTarget {
    fn drop(&mut self) {
        // SAFETY: ends the connection opened in `open`; the library is still loaded.
        unsafe { (self.api.disconnect)() };
    }
}
