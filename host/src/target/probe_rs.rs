//! Target access through the probe-rs library.

use super::{Connection, Result, Target};
use probe_rs::architecture::arm::SwoConfig;
use probe_rs::architecture::arm::component::TraceSink;
use probe_rs::probe::DebugProbeSelector;
use probe_rs::probe::list::Lister;
use probe_rs::{MemoryInterface, Permissions, Session};

pub struct ProbeRsTarget {
    session: Session,
    speed_khz: u32,
}

impl ProbeRsTarget {
    pub fn open(connection: &Connection) -> Result<Self> {
        let chip = connection.chip.as_deref().ok_or("probe-rs needs --chip")?;
        let lister = Lister::new();
        let mut probe = match &connection.probe {
            Some(selector) => lister.open(selector.parse::<DebugProbeSelector>()?)?,
            None => {
                let probes = lister.list_all();
                match probes.as_slice() {
                    [only] => only.open()?,
                    [] => return Err("no probe found".into()),
                    _ => return Err("several probes found: pick one with --probe-id".into()),
                }
            }
        };
        let speed_khz = probe.set_speed(connection.speed_khz)?;
        let session = probe.attach(chip, Permissions::default())?;
        Ok(Self { session, speed_khz })
    }
}

impl Target for ProbeRsTarget {
    fn speed_khz(&self) -> Option<u32> {
        Some(self.speed_khz)
    }

    fn read_words(&mut self, address: u32, words: &mut [u32]) -> Result<()> {
        self.session.core(0)?.read_32(address.into(), words)?;
        Ok(())
    }

    fn write_word(&mut self, address: u32, value: u32) -> Result<()> {
        self.session.core(0)?.write_word_32(address.into(), value)?;
        Ok(())
    }

    fn swo_start(&mut self, tpiu_clock_hz: u32, baud: u32) -> Result<()> {
        let config = SwoConfig::new(tpiu_clock_hz).set_baud(baud);
        self.session.setup_tracing(0, TraceSink::Swo(config))?;
        Ok(())
    }

    fn swo_read(&mut self, bytes: &mut Vec<u8>) -> Result<()> {
        bytes.extend(self.session.read_trace_data()?);
        Ok(())
    }
}
