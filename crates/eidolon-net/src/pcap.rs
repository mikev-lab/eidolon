//! Lightweight, zero-dependency PCAP (libpcap 2.4) file writer and packet synthesizer.
//!
//! Provides the ability to generate standard `.pcap` capture streams readable by Wireshark
//! and tcpdump directly from simulation traffic, validating Layer 2 through Layer 7 framing.

use std::io::{self, Write};

/// Standard PCAP magic number for identical endianness (microsecond resolution).
pub const PCAP_MAGIC_NUMBER: u32 = 0xA1B2C3D4;
/// Major version of the libpcap file format.
pub const PCAP_VERSION_MAJOR: u16 = 2;
/// Minor version of the libpcap file format.
pub const PCAP_VERSION_MINOR: u16 = 4;
/// Standard snaplen indicating full packet capture without truncation.
pub const PCAP_SNAPLEN: u32 = 65535;
/// Link-layer header type 1: Ethernet (10Mb, 100Mb, 1000Mb, and up).
pub const LINKTYPE_ETHERNET: u32 = 1;

/// Standard Ethernet II frame header length (14 bytes: 6B dst, 6B src, 2B type).
pub const ETHERNET_HEADER_LEN: usize = 14;
/// Standard IPv4 header length without options (20 bytes).
pub const IPV4_HEADER_LEN: usize = 20;
/// Standard UDP header length (8 bytes).
pub const UDP_HEADER_LEN: usize = 8;
/// Total Layer 2, Layer 3, and Layer 4 framing overhead combined (42 bytes).
pub const L2_TO_L4_OVERHEAD: usize = ETHERNET_HEADER_LEN + IPV4_HEADER_LEN + UDP_HEADER_LEN;

/// Zero-dependency PCAP capture stream writer.
#[derive(Debug)]
pub struct PcapWriter<W: Write> {
    writer: W,
    packets_written: u64,
    bytes_written: u64,
}

impl<W: Write> PcapWriter<W> {
    /// Creates a new PCAP writer and emits the standard 24-byte libpcap global file header.
    pub fn new(mut writer: W) -> io::Result<Self> {
        let mut global_header = [0u8; 24];
        global_header[0..4].copy_from_slice(&PCAP_MAGIC_NUMBER.to_ne_bytes());
        global_header[4..6].copy_from_slice(&PCAP_VERSION_MAJOR.to_ne_bytes());
        global_header[6..8].copy_from_slice(&PCAP_VERSION_MINOR.to_ne_bytes());
        // thiszone (GMT to local correction, 0)
        global_header[8..12].copy_from_slice(&0u32.to_ne_bytes());
        // sigfigs (accuracy of timestamps, 0)
        global_header[12..16].copy_from_slice(&0u32.to_ne_bytes());
        // snaplen (max octets per captured packet)
        global_header[16..20].copy_from_slice(&PCAP_SNAPLEN.to_ne_bytes());
        // network link type (1 = Ethernet)
        global_header[20..24].copy_from_slice(&LINKTYPE_ETHERNET.to_ne_bytes());

        writer.write_all(&global_header)?;
        Ok(Self {
            writer,
            packets_written: 0,
            bytes_written: 24,
        })
    }

    /// Writes a synthetic Ethernet/IPv4/UDP packet containing the application payload.
    pub fn write_udp_packet(
        &mut self,
        timestamp_micros: u64,
        src_ip: [u8; 4],
        dst_ip: [u8; 4],
        src_port: u16,
        dst_port: u16,
        payload: &[u8],
    ) -> io::Result<usize> {
        let frame_len = L2_TO_L4_OVERHEAD + payload.len();
        let mut frame = [0u8; 2048];
        if frame_len > frame.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Payload exceeds maximum frame buffer capacity",
            ));
        }

        // 1. Ethernet Header (14 bytes)
        // Dest MAC: 02:00:00:00:00:02 (Locally administered unicast)
        frame[0..6].copy_from_slice(&[0x02, 0x00, 0x00, 0x00, 0x00, 0x02]);
        // Src MAC: 02:00:00:00:00:01
        frame[6..12].copy_from_slice(&[0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);
        // EtherType: 0x0800 (IPv4)
        frame[12..14].copy_from_slice(&0x0800u16.to_be_bytes());

        // 2. IPv4 Header (20 bytes)
        let ip_total_len = (IPV4_HEADER_LEN + UDP_HEADER_LEN + payload.len()) as u16;
        frame[14] = 0x45; // Version 4, IHL 5 (20 bytes)
        frame[15] = 0x00; // DSCP / ECN
        frame[16..18].copy_from_slice(&ip_total_len.to_be_bytes());
        frame[18..20].copy_from_slice(&0x0000u16.to_be_bytes()); // Identification
        frame[20..22].copy_from_slice(&0x4000u16.to_be_bytes()); // Flags: Don't Fragment
        frame[22] = 64; // TTL
        frame[23] = 17; // Protocol: 17 (UDP)
        frame[24..26].copy_from_slice(&0x0000u16.to_be_bytes()); // Checksum placeholder
        frame[26..30].copy_from_slice(&src_ip);
        frame[30..34].copy_from_slice(&dst_ip);

        // Compute and insert IPv4 header checksum (RFC 1071)
        let checksum = compute_ipv4_checksum(&frame[14..34]);
        frame[24..26].copy_from_slice(&checksum.to_be_bytes());

        // 3. UDP Header (8 bytes)
        let udp_len = (UDP_HEADER_LEN + payload.len()) as u16;
        frame[34..36].copy_from_slice(&src_port.to_be_bytes());
        frame[36..38].copy_from_slice(&dst_port.to_be_bytes());
        frame[38..40].copy_from_slice(&udp_len.to_be_bytes());
        frame[40..42].copy_from_slice(&0x0000u16.to_be_bytes()); // UDP checksum optional in IPv4

        // 4. Copy application payload
        frame[42..frame_len].copy_from_slice(payload);

        // 5. Write PCAP Packet Record Header (16 bytes)
        let ts_sec = (timestamp_micros / 1_000_000) as u32;
        let ts_usec = (timestamp_micros % 1_000_000) as u32;
        let incl_len = frame_len as u32;
        let orig_len = frame_len as u32;

        let mut record_header = [0u8; 16];
        record_header[0..4].copy_from_slice(&ts_sec.to_ne_bytes());
        record_header[4..8].copy_from_slice(&ts_usec.to_ne_bytes());
        record_header[8..12].copy_from_slice(&incl_len.to_ne_bytes());
        record_header[12..16].copy_from_slice(&orig_len.to_ne_bytes());

        self.writer.write_all(&record_header)?;
        self.writer.write_all(&frame[..frame_len])?;

        self.packets_written += 1;
        self.bytes_written += 16 + frame_len as u64;

        Ok(frame_len)
    }

    /// Flushes the underlying writer.
    pub fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }

    /// Returns the total number of packets written.
    #[inline]
    pub const fn packets_written(&self) -> u64 {
        self.packets_written
    }

    /// Returns the total bytes written (including headers).
    #[inline]
    pub const fn bytes_written(&self) -> u64 {
        self.bytes_written
    }
}

/// Computes the standard RFC 1071 one's complement Internet Checksum for an IPv4 header.
fn compute_ipv4_checksum(header: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut i = 0;
    while i < header.len() {
        let word = u16::from_be_bytes([header[i], header[i + 1]]);
        sum = sum.wrapping_add(word as u32);
        i += 2;
    }

    while (sum >> 16) > 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    !(sum as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pcap_global_header_initialization() {
        let mut buffer = Vec::new();
        let writer = PcapWriter::new(&mut buffer).expect("Initialization should succeed");

        assert_eq!(writer.packets_written(), 0);
        assert_eq!(writer.bytes_written(), 24);
        assert_eq!(buffer.len(), 24);

        // Verify magic number
        let magic = u32::from_ne_bytes(buffer[0..4].try_into().unwrap());
        assert_eq!(magic, PCAP_MAGIC_NUMBER);

        let major = u16::from_ne_bytes(buffer[4..6].try_into().unwrap());
        let minor = u16::from_ne_bytes(buffer[6..8].try_into().unwrap());
        assert_eq!(major, 2);
        assert_eq!(minor, 4);

        let linktype = u32::from_ne_bytes(buffer[20..24].try_into().unwrap());
        assert_eq!(linktype, LINKTYPE_ETHERNET);
    }

    #[test]
    fn test_pcap_packet_record_and_framing() {
        let mut buffer = Vec::new();
        let mut writer = PcapWriter::new(&mut buffer).expect("Init");

        let payload = b"eidolon_wire_test";
        let frame_len = writer
            .write_udp_packet(
                1_500_000, // 1.5s
                [127, 0, 0, 1],
                [127, 0, 0, 1],
                7777,
                8888,
                payload,
            )
            .expect("Write packet");

        assert_eq!(frame_len, L2_TO_L4_OVERHEAD + payload.len());
        assert_eq!(writer.packets_written(), 1);

        // 24 global header + 16 record header + frame_len
        assert_eq!(buffer.len(), 24 + 16 + frame_len);

        // Validate timestamp
        let ts_sec = u32::from_ne_bytes(buffer[24..28].try_into().unwrap());
        let ts_usec = u32::from_ne_bytes(buffer[28..32].try_into().unwrap());
        assert_eq!(ts_sec, 1);
        assert_eq!(ts_usec, 500_000);

        // Validate EtherType at offset 24 + 16 + 12 = 52
        let ethertype = u16::from_be_bytes(buffer[52..54].try_into().unwrap());
        assert_eq!(ethertype, 0x0800);

        // Validate IP Protocol at offset 54 + 9 = 63
        assert_eq!(buffer[63], 17); // UDP
    }
}
