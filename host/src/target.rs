//! Access to the memory of a running target through a debug probe.

pub mod jlink;
pub mod probe_rs;
pub mod qemu;
pub mod stlink;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Word access to target memory while the core runs. Every access is a
/// 32-bit access, so a word the target writes atomically is read atomically.
pub trait Target {
    fn read_words(&mut self, address: u32, words: &mut [u32]) -> Result<()>;
    fn write_word(&mut self, address: u32, value: u32) -> Result<()>;

    /// The debug clock the probe runs at, in kHz: the one asked for, or the
    /// closest the probe supports. None where there is no debug clock.
    fn speed_khz(&self) -> Option<u32> {
        None
    }

    fn read_word(&mut self, address: u32) -> Result<u32> {
        let mut word = [0u32];
        self.read_words(address, &mut word)?;
        Ok(word[0])
    }
}

/// " at <n> kHz", the debug clock the target's probe runs at, for a summary;
/// empty where there is none.
pub fn speed_text(target: &dyn Target) -> String {
    target
        .speed_khz()
        .map_or_else(String::new, |khz| format!(" at {khz} kHz"))
}

/// The ways the host reaches a probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// The probe-rs library, talking to the probe over USB itself.
    ProbeRs,
    /// SEGGER's J-Link library.
    JLink,
    /// ST's STM32CubeProgrammer library, for ST-Link probes.
    StLink,
    /// QEMU's monitor, for tests without a probe.
    Qemu,
}

impl Backend {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "probe-rs" => Some(Self::ProbeRs),
            "jlink" => Some(Self::JLink),
            "stlink" => Some(Self::StLink),
            "qemu" => Some(Self::Qemu),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::ProbeRs => "probe-rs",
            Self::JLink => "jlink",
            Self::StLink => "stlink",
            Self::Qemu => "qemu",
        }
    }
}

/// What it takes to open a probe, whatever the backend.
#[derive(Clone, Debug)]
pub struct Connection {
    pub backend: Backend,
    /// probe-rs: a selector (`VID:PID[:SERIAL]`); J-Link and ST-Link: the
    /// probe serial number, optional when a single probe is attached;
    /// QEMU: the monitor's `<host>:<port>`.
    pub probe: Option<String>,
    /// The chip name, as probe-rs or J-Link knows it. Unused by ST-Link.
    pub chip: Option<String>,
    pub speed_khz: u32,
}

/// Opens the probe and attaches to the running target without halting or
/// resetting it.
pub fn open(connection: &Connection) -> Result<Box<dyn Target>> {
    match connection.backend {
        Backend::ProbeRs => Ok(Box::new(probe_rs::ProbeRsTarget::open(connection)?)),
        Backend::JLink => Ok(Box::new(jlink::JLinkTarget::open(connection)?)),
        Backend::StLink => Ok(Box::new(stlink::StLinkTarget::open(connection)?)),
        Backend::Qemu => Ok(Box::new(qemu::QemuTarget::open(connection)?)),
    }
}
