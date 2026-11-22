//! Decoding the bench events the target writes to SWO: ITM packets, then
//! the four-word events carried by stimulus port 1 (see
//! target/bench/bench.cpp).

const PORT: u8 = 1;
const EVENT_TYPE: u32 = 1;

/// What the decoder has seen so far.
#[derive(Clone, Copy, Debug, Default)]
pub struct Counts {
    /// Events decoded intact.
    pub received: u64,
    /// Stimulus words that did not fit an event: a protocol or probe fault.
    pub corrupt: u64,
    /// ITM overflow packets: the target dropped trace data.
    pub overflows: u64,
    pub bytes: u64,
}

enum State {
    Header,
    /// Payload bytes of a source packet still to come.
    Source {
        port: u8,
        hardware: bool,
        remaining: u8,
        value: u32,
        shift: u32,
    },
    /// Continuation bytes of a timestamp or extension packet.
    Continuation,
    /// Zeros of a synchronization packet, ended by 0x80.
    Sync,
}

pub struct Decoder {
    state: State,
    words: Vec<u32>,
    /// Only events in `from..until` are counted.
    from: u32,
    until: Option<u32>,
    pub counts: Counts,
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            state: State::Header,
            words: Vec::new(),
            from: 0,
            until: None,
            counts: Counts::default(),
        }
    }

    /// Clears the counts and counts only events from `index` on.
    pub fn restart(&mut self, index: u32) {
        self.from = index;
        self.until = None;
        self.counts = Counts::default();
    }

    /// Counts no event from `index` on.
    pub fn until(&mut self, index: u32) {
        self.until = Some(index);
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.counts.bytes += bytes.len() as u64;
        for &byte in bytes {
            self.byte(byte);
        }
    }

    fn byte(&mut self, byte: u8) {
        match &mut self.state {
            State::Header => self.header(byte),
            State::Source {
                port,
                hardware,
                remaining,
                value,
                shift,
            } => {
                *value |= u32::from(byte) << *shift;
                *shift += 8;
                *remaining -= 1;
                if *remaining == 0 {
                    let (port, hardware, value) = (*port, *hardware, *value);
                    self.state = State::Header;
                    if !hardware && port == PORT {
                        self.word(value);
                    }
                }
            }
            State::Continuation => {
                if byte & 0x80 == 0 {
                    self.state = State::Header;
                }
            }
            State::Sync => {
                if byte != 0 {
                    self.state = State::Header;
                }
            }
        }
    }

    fn header(&mut self, byte: u8) {
        let size = byte & 0b11;
        if size != 0 {
            // Source packet: 1, 2 or 4 bytes of payload.
            self.state = State::Source {
                port: byte >> 3,
                hardware: byte & 0b100 != 0,
                remaining: if size == 3 { 4 } else { size },
                value: 0,
                shift: 0,
            };
        } else if byte == 0 {
            self.state = State::Sync;
        } else if byte == 0x70 {
            self.counts.overflows += 1;
        } else if byte & 0x80 != 0 {
            // Timestamp, extension or global timestamp with payload.
            self.state = State::Continuation;
        }
        // Otherwise a one-byte timestamp or extension: nothing follows.
    }

    /// Frames stimulus words into events: type, timestamp, index, ~index.
    fn word(&mut self, word: u32) {
        self.words.push(word);
        while self.words.len() >= 4 {
            let event = &self.words[..4];
            if event[0] == EVENT_TYPE && event[3] == !event[2] {
                let index = event[2];
                self.words.drain(..4);
                self.event(index);
            } else {
                self.words.remove(0);
                self.counts.corrupt += 1;
            }
        }
    }

    fn event(&mut self, index: u32) {
        let offset = index.wrapping_sub(self.from);
        let before_end = self.until.is_none_or(|until| offset < until.wrapping_sub(self.from));
        if offset < 1 << 31 && before_end {
            self.counts.received += 1;
        }
    }
}
