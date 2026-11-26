use super::*;

fn value(comparator: u8, value: u32, size: u8) -> Packet {
    Packet::DataValue {
        comparator,
        write: true,
        value,
        size,
    }
}

fn timestamp(delta: u32) -> Packet {
    Packet::Timestamp { delta, control: 0 }
}

#[test]
fn reads_data_values_of_each_size() {
    // Hardware source, discriminator 0b10101 (comparator 2, write), 4 bytes;
    // 0b10011 (comparator 1, write), 2 bytes; 0b10000 (comparator 0, read), 1 byte.
    let bytes = [0xaf, 0x78, 0x56, 0x34, 0x12, 0x9e, 0x34, 0x12, 0x85, 0x7f];
    let (packets, cut) = packets(&bytes);
    assert!(!cut);
    assert_eq!(
        packets,
        [
            value(2, 0x1234_5678, 4),
            value(1, 0x1234, 2),
            Packet::DataValue {
                comparator: 0,
                write: false,
                value: 0x7f,
                size: 1
            },
        ]
    );
}

#[test]
fn reads_local_timestamps_of_both_forms() {
    // One byte: 0b0ddd0000. Continued: 0b11cc0000, then 7 bits a byte, low first.
    let bytes = [0x30, 0xc0, 0x85, 0x01, 0xe0, 0x7f, 0xc0, 0xff, 0xff, 0xff, 0x7f];
    let (packets, cut) = packets(&bytes);
    assert!(!cut);
    assert_eq!(
        packets,
        [
            timestamp(3),
            timestamp(0x85),
            Packet::Timestamp {
                delta: 0x7f,
                control: 2
            },
            timestamp(0x0fff_ffff),
        ]
    );
}

#[test]
fn reads_sync_and_overflow() {
    let bytes = [0, 0, 0, 0, 0, 0x80, 0x70, 0, 0, 0x80];
    assert_eq!(
        packets(&bytes),
        (vec![Packet::Sync, Packet::Overflow, Packet::Malformed, Packet::Malformed], false)
    );
}

#[test]
fn reads_past_other_packets() {
    let bytes = [
        0x01, 0xaa, // instrumentation port 0, one byte
        0x0e, 0x01, 0x00, // hardware source 1 (exception trace), two bytes
        0x94, 0x81, 0x01, // global timestamp 1
        0xb4, 0x01, // global timestamp 2
        0x08, // extension, one byte
        0x88, 0x80, 0x01, // extension, continued
        0xaf, 1, 0, 0, 0,
    ];
    let (packets, cut) = packets(&bytes);
    assert!(!cut);
    assert_eq!(packets, [[Packet::Other; 6].as_slice(), &[value(2, 1, 4)]].concat());
}

#[test]
fn reserved_headers_are_malformed() {
    for header in [0x04, 0x14, 0x84, 0x80, 0xa0] {
        assert_eq!(packets(&[header]).0, [Packet::Malformed], "{header:#04x}");
    }
}

#[test]
fn a_packet_cut_at_the_end_is_left_out() {
    for bytes in [
        &[0x30, 0xaf, 0x01, 0x02][..],
        &[0x30, 0xc0, 0x81],
        &[0x30, 0, 0, 0],
        &[0x30, 0x94, 0x80],
    ] {
        assert_eq!(packets(bytes), (vec![timestamp(3)], true), "{bytes:02x?}");
    }
}
