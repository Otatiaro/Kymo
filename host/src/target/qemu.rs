//! Target access through QEMU's monitor (QMP), for tests without a probe.
//!
//! The monitor reads guest memory while the CPU keeps running, as a probe
//! does: `xp` prints the words, each read as one aligned 32-bit access.
//! QEMU must be started with `-qmp tcp:<host>:<port>,server=on,wait=off`,
//! and `--probe-id` gives `<host>:<port>`. The monitor cannot write guest
//! memory.

use super::{Connection, Result, Target};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

pub struct QemuTarget {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl QemuTarget {
    pub fn open(connection: &Connection) -> Result<Self> {
        let address = connection
            .probe
            .as_deref()
            .ok_or("qemu needs --probe-id <host>:<port>")?;
        let writer = TcpStream::connect(address).map_err(|error| format!("{address}: {error}"))?;
        // One command, one reply: waiting to coalesce small writes only delays it.
        writer.set_nodelay(true)?;
        let mut target = Self {
            reader: BufReader::new(writer.try_clone()?),
            writer,
        };
        let mut greeting = String::new();
        target.reader.read_line(&mut greeting)?;
        if !greeting.contains("\"QMP\"") {
            return Err(format!("{address} is not a QMP monitor").into());
        }
        target.execute(r#"{"execute":"qmp_capabilities"}"#)?;
        Ok(target)
    }

    /// Sends one command and returns its reply, skipping asynchronous events.
    fn execute(&mut self, command: &str) -> Result<String> {
        self.writer.write_all(format!("{command}\n").as_bytes())?;
        loop {
            let mut line = String::new();
            if self.reader.read_line(&mut line)? == 0 {
                return Err("QEMU closed the monitor".into());
            }
            if line.starts_with(r#"{"return""#) {
                return Ok(line);
            }
            if line.starts_with(r#"{"error""#) {
                return Err(format!("QMP: {}", line.trim_end()).into());
            }
        }
    }
}

impl Target for QemuTarget {
    fn read_words(&mut self, address: u32, words: &mut [u32]) -> Result<()> {
        if words.is_empty() {
            return Ok(());
        }
        let reply = self.execute(&format!(
            r#"{{"execute":"human-monitor-command","arguments":{{"command-line":"xp /{}wx {address:#x}"}}}}"#,
            words.len()
        ))?;
        // The reply's text is a JSON string: lines "<address>: 0x<word> ...",
        // separated by the escaped "\r\n".
        let mut read = 0;
        for line in reply.split("\\n") {
            let Some((_, values)) = line.split_once(": ") else {
                continue;
            };
            for value in values.split_whitespace() {
                let Some(hex) = value.strip_prefix("0x") else { continue };
                let hex = hex.trim_end_matches(|c: char| !c.is_ascii_hexdigit());
                let word = u32::from_str_radix(hex, 16).map_err(|_| format!("QMP: unexpected xp output {line}"))?;
                *words.get_mut(read).ok_or("QMP: xp printed more words than asked")? = word;
                read += 1;
            }
        }
        if read != words.len() {
            return Err(format!("QMP: xp printed {read} words of {}", words.len()).into());
        }
        Ok(())
    }
}
