//! The ITM's packets, as SWO carries them (ARMv7-M and ARMv8-M architecture
//! reference manuals, ITM and DWT packet protocol).
//!
//! A packet starts with a header byte that tells its kind and its size. The
//! packets Kymo reads are the DWT's data values, each sent when a comparator
//! matches a write, and the local timestamps the ITM sends after them. Global
//! timestamps, extension packets and the instrumentation ports' packets are
//! read past. A header the protocol reserves, or a synchronisation packet cut
//! short, is malformed: the bytes that follow it may not start a packet.

/// A packet of the stream, read past or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Packet {
    /// A synchronisation packet: at least 47 zero bits, then a one.
    Sync,
    /// The ITM lost packets: its output was full.
    Overflow,
    /// The cycles since the last local timestamp, and its timestamp control:
    /// 0 when the timestamp and the packets it dates are synchronous, else
    /// delayed.
    Timestamp { delta: u32, control: u8 },
    /// A data value a DWT comparator sent: its number, whether the access
    /// was a write, its value and its size in bytes.
    DataValue {
        comparator: u8,
        write: bool,
        value: u32,
        size: u8,
    },
    /// A packet read past that the next local timestamp dates too: one of an
    /// instrumentation port or of another hardware source.
    Source,
    /// A packet read past that no local timestamp dates: a global timestamp
    /// or an extension.
    Other,
    /// A header the protocol reserves, or zeros not ending a synchronisation
    /// packet.
    Malformed,
}

/// The packets of `bytes` in order, and whether the last one is cut short at
/// the end of the bytes, then left out.
pub fn packets(bytes: &[u8]) -> (Vec<Packet>, bool) {
    let mut packets = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let Some((packet, length)) = packet(&bytes[at..]) else {
            return (packets, true);
        };
        packets.push(packet);
        at += length;
    }
    (packets, false)
}

/// The packet at the start of `bytes` and its length; none when `bytes` end
/// inside it.
fn packet(bytes: &[u8]) -> Option<(Packet, usize)> {
    let header = bytes[0];
    if header == 0 {
        let zeros = bytes.iter().take_while(|&&byte| byte == 0).count();
        let end = *bytes.get(zeros)?;
        // 47 zero bits then a one: five zero bytes, and a sixth whose low
        // seven bits are zero.
        return Some(if zeros >= 5 && end == 0x80 {
            (Packet::Sync, zeros + 1)
        } else {
            (Packet::Malformed, zeros)
        });
    }
    if header == 0x70 {
        return Some((Packet::Overflow, 1));
    }
    let size = header & 0b11;
    if size != 0 {
        let length = usize::from(1u8 << (size - 1));
        let payload = bytes.get(1..=length)?;
        let value = payload
            .iter()
            .rev()
            .fold(0u32, |value, byte| value << 8 | u32::from(*byte));
        let id = header >> 3;
        // Hardware source, discriminator 0b10nnw: a data value of comparator nn.
        let packet = if header & 0b100 != 0 && id >> 3 == 0b10 {
            Packet::DataValue {
                comparator: (id >> 1) & 0b11,
                write: id & 1 != 0,
                value,
                size: length as u8,
            }
        } else {
            Packet::Source
        };
        return Some((packet, 1 + length));
    }
    if header & 0x0f == 0 {
        if header & 0x80 == 0 {
            // A one-byte local timestamp, synchronous: 0b0ddd0000, ddd 1 to 6.
            return Some((
                Packet::Timestamp {
                    delta: u32::from(header >> 4),
                    control: 0,
                },
                1,
            ));
        }
        if header & 0xc0 == 0xc0 {
            let (delta, length) = continued(&bytes[1..], 4)?;
            return Some((
                Packet::Timestamp {
                    delta,
                    control: (header >> 4) & 0b11,
                },
                1 + length,
            ));
        }
        return Some((Packet::Malformed, 1));
    }
    if header == 0x94 || header == 0xb4 {
        let (_, length) = continued(&bytes[1..], usize::MAX)?;
        return Some((Packet::Other, 1 + length));
    }
    if header & 0x0b == 0x08 {
        // An extension: its continuation bit is the header's top bit.
        if header & 0x80 == 0 {
            return Some((Packet::Other, 1));
        }
        let (_, length) = continued(&bytes[1..], usize::MAX)?;
        return Some((Packet::Other, 1 + length));
    }
    Some((Packet::Malformed, 1))
}

/// The value of the bytes at the start of `bytes`, seven bits each, low
/// first, up to the first without its continuation bit and at most `most`,
/// and how many there are; none when `bytes` end before. The last byte
/// allowed ends the value whatever its top bit.
fn continued(bytes: &[u8], most: usize) -> Option<(u32, usize)> {
    let mut value = 0u32;
    for (n, &byte) in bytes.iter().enumerate() {
        if n < 4 {
            value |= u32::from(byte & 0x7f) << (7 * n);
        }
        if byte & 0x80 == 0 || n + 1 == most {
            return Some((value, n + 1));
        }
    }
    None
}

#[cfg(test)]
mod tests;
