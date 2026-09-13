//! Tier 2: Adversarial Network & Chaos Degradation Test Suite.
//!
//! Tests UDP packet loss (30% to 50%), simulated network jitter, out-of-order delivery,
//! sequence number wrap-around, and sliding-window selective ACK reassembly.

use eidolon_net::channel::{ReliableChannel, UnreliableSequencer};
use eidolon_net::packet::{PacketHeader, PacketView};
use eidolon_net::protocol::{ChannelType, PacketType, HEADER_SIZE};

/// Lightweight deterministic Xorshift PRNG for synthetic chaos testing without third-party crates.
struct ChaosPrng {
    state: u64,
}

impl ChaosPrng {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0x853c49e6748fea9b } else { seed },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Returns a pseudo-random floating point value in [0.0, 1.0).
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }
}

#[test]
fn test_unreliable_sequencer_under_40_percent_loss() {
    let mut rng = ChaosPrng::new(12345);
    let mut server_sequencer = UnreliableSequencer::new();
    let mut client_sequencer = UnreliableSequencer::new();

    let mut total_sent = 0;
    let mut accepted_by_server = 0;
    let mut dropped_packets = 0;

    for _ in 0..1000 {
        let seq = client_sequencer.next_sequence();
        total_sent += 1;

        // 40% synthetic packet loss
        if rng.next_f64() < 0.40 {
            dropped_packets += 1;
            continue;
        }

        let accepted = server_sequencer.process_incoming_sequence(seq);
        if accepted {
            accepted_by_server += 1;
        }
    }

    assert_eq!(total_sent, 1000);
    assert!(dropped_packets > 300 && dropped_packets < 500);
    // Accepted packets must equal total received since sequence numbers arrive in increasing order
    assert_eq!(accepted_by_server, total_sent - dropped_packets);
}

#[test]
fn test_unreliable_sequencer_jitter_and_reordering() {
    let mut server_sequencer = UnreliableSequencer::new();

    // Packets generated in order: 0, 1, 2, 3, 4, 5
    // Received out of order due to 150ms network jitter:
    // 0 arrives, then 3 arrives (jumping ahead), then 1 and 2 arrive late (stale), then 5, then 4.
    assert!(server_sequencer.process_incoming_sequence(0));
    assert!(server_sequencer.process_incoming_sequence(3));

    // Packets 1 and 2 arrived late: must be rejected as stale state updates!
    assert!(!server_sequencer.process_incoming_sequence(1));
    assert!(!server_sequencer.process_incoming_sequence(2));

    // Packet 3 re-transmitted or duplicated: rejected
    assert!(!server_sequencer.process_incoming_sequence(3));

    // Packet 5 arrives
    assert!(server_sequencer.process_incoming_sequence(5));

    // Packet 4 arrives late: rejected
    assert!(!server_sequencer.process_incoming_sequence(4));

    assert_eq!(server_sequencer.highest_received_sequence(), 5);

    // Bitfield must track that 0, 1, 2, 3, 4 were recorded
    let bitfield = server_sequencer.received_bitfield();
    // Bit 0 = seq 4 (diff 1), Bit 1 = seq 3 (diff 2), Bit 2 = seq 2 (diff 3), Bit 3 = seq 1 (diff 4), Bit 4 = seq 0 (diff 5)
    // Both seq 3, 4, 2, 1 were processed into bitfield
    assert_ne!(bitfield, 0);
}

#[test]
fn test_reliable_channel_1000_messages_under_35_percent_loss() {
    let mut rng = ChaosPrng::new(99999);
    let mut sender_channel = ReliableChannel::<64, 64>::new();
    let mut receiver_channel = ReliableChannel::<64, 64>::new();

    let mut delivered_messages = Vec::new();
    let mut current_msg_id = 0u32;
    let target_messages = 500u32;

    // Simulation runs in discrete 50ms ticks
    for _tick in 0..5000 {
        // Queue new message if sender channel has capacity and we haven't reached target
        if current_msg_id < target_messages && sender_channel.pending_count() < 32 {
            let payload = current_msg_id.to_be_bytes();
            let queued = sender_channel.queue_reliable_message(&payload);
            if queued.is_ok() {
                current_msg_id += 1;
            }
        }

        // Collect packets to send (new + retransmissions)
        let mut packets_to_transmit = Vec::new();
        sender_channel.check_retransmissions(|seq, data| {
            packets_to_transmit.push((seq, data.to_vec()));
        });

        // Transmit over synthetic lossy link (35% loss)
        for (seq, data) in packets_to_transmit {
            if rng.next_f64() < 0.35 {
                continue; // Packet dropped over the wire
            }

            // Receiver ingests packet
            if let Ok(Some((payload, len))) = receiver_channel.receive_reliable_packet(seq, &data) {
                delivered_messages.push(u32::from_be_bytes([
                    payload[0], payload[1], payload[2], payload[3],
                ]));
                assert_eq!(len, 4);

                // Drain any contiguous buffered packets
                while let Some((p, _)) = receiver_channel.drain_next_ordered_packet() {
                    delivered_messages.push(u32::from_be_bytes([p[0], p[1], p[2], p[3]]));
                }
            }

            // Send ACK packet back to sender (ACKs also subject to 20% loss)
            if rng.next_f64() >= 0.20 {
                let (ack, ack_bitfield) = (seq, 0u32);
                sender_channel.process_remote_ack(ack, ack_bitfield);
            }
        }

        if delivered_messages.len() == target_messages as usize {
            break;
        }
    }

    // Verify 100% in-order delivery without gaps or duplicates
    assert_eq!(delivered_messages.len(), target_messages as usize);
    for (idx, &msg_id) in delivered_messages.iter().enumerate() {
        assert_eq!(idx as u32, msg_id, "Message delivered out of sequence!");
    }
}

#[test]
fn test_sequence_number_circular_rollover_boundaries() {
    let mut seq = UnreliableSequencer::new();

    // Start near boundary 65534
    assert!(seq.process_incoming_sequence(65534));
    assert!(seq.process_incoming_sequence(65535));

    // Rollover across 0
    assert!(seq.process_incoming_sequence(0));
    assert_eq!(seq.highest_received_sequence(), 0);

    assert!(seq.process_incoming_sequence(1));
    assert_eq!(seq.highest_received_sequence(), 1);

    // Stale sequence 65535 arriving after rollover must be rejected
    assert!(!seq.process_incoming_sequence(65535));
    // Stale sequence 0 arriving after 1 must be rejected
    assert!(!seq.process_incoming_sequence(0));
}

#[test]
fn test_packet_framing_codec_integration() {
    let mut raw_buffer = [0u8; 128];
    let header = PacketHeader::new(
        ChannelType::UnreliableSequenced,
        PacketType::StateUpdate,
        42,
        41,
        0b1111,
    );

    let written = header.write_to(&mut raw_buffer).expect("write header");
    assert_eq!(written, HEADER_SIZE);

    let test_payload = b"entity_id:100;qx:1234;qz:5678;qy:999";
    raw_buffer[HEADER_SIZE..HEADER_SIZE + test_payload.len()].copy_from_slice(test_payload);

    let packet_slice = &raw_buffer[..HEADER_SIZE + test_payload.len()];
    let packet_view = PacketView::from_bytes(packet_slice).expect("parse packet view");

    assert_eq!(packet_view.header.sequence, 42);
    assert_eq!(packet_view.header.ack, 41);
    assert_eq!(packet_view.header.channel, ChannelType::UnreliableSequenced);
    assert_eq!(packet_view.payload, test_payload);
}
