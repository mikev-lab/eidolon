//! WebAssembly and WebTransport Client Engine Tier.
//!
//! Provides an allocation-free, target-agnostic client engine that runs directly inside
//! modern web browsers via WebAssembly and WebTransport / WebSocket binary datagrams.

use std::time::Instant;

use eidolon_core::fixed::Vec3Fix;
use eidolon_core::quant::{QuantizedCellCoord, QuantizedYaw};
use eidolon_net::error::NetError;
use eidolon_net::packet::PacketHeader;
use eidolon_net::protocol::{ChannelType, PacketType, HEADER_SIZE, PROTOCOL_VERSION};

use crate::client::ClientError;
use crate::config::ClientConfig;
use crate::event::ClientEvent;
use crate::world_view::ClientWorldView;

/// Maximum renderable entities returned in a single batch to WebGL / Canvas.
pub const MAX_WEB_RENDER_ENTITIES: usize = 16384;

/// C-compatible representation of an extrapolated entity transform for browser rendering.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WebRenderEntity {
    /// Server entity identifier.
    pub entity_id: u32,
    /// Render X position in meters.
    pub x: f32,
    /// Render Y position (elevation) in meters.
    pub y: f32,
    /// Render Z position in meters.
    pub z: f32,
    /// Yaw angle in degrees [0.0, 360.0).
    pub yaw_degrees: f32,
    /// Entity state flags (walking, sprinting, jumping, combat).
    pub flags: u8,
}

impl Default for WebRenderEntity {
    fn default() -> Self {
        Self {
            entity_id: 0,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            yaw_degrees: 0.0,
            flags: 0,
        }
    }
}

/// Standalone WebAssembly client engine running inside browser environments.
pub struct WasmClient {
    config: ClientConfig,
    world_view: ClientWorldView,
    sequence: u16,
    recv_seq: u16,
    ack_mask: u32,
    event_queue: [Option<ClientEvent>; 32],
    event_head: usize,
    event_tail: usize,
    event_count: usize,
    render_entities: Box<[WebRenderEntity; MAX_WEB_RENDER_ENTITIES]>,
    render_entities_count: usize,
}

impl WasmClient {
    /// Creates a new WebAssembly client instance with default parameters.
    pub fn new(config: ClientConfig) -> Self {
        let render_entities: Box<[WebRenderEntity; MAX_WEB_RENDER_ENTITIES]> =
            match vec![WebRenderEntity::default(); MAX_WEB_RENDER_ENTITIES]
                .into_boxed_slice()
                .try_into()
            {
                Ok(boxed_arr) => boxed_arr,
                Err(_) => Box::new([WebRenderEntity::default(); MAX_WEB_RENDER_ENTITIES]),
            };

        Self {
            config,
            world_view: ClientWorldView::new(),
            sequence: 0,
            recv_seq: 0,
            ack_mask: 0,
            event_queue: [const { None }; 32],
            event_head: 0,
            event_tail: 0,
            event_count: 0,
            render_entities,
            render_entities_count: 0,
        }
    }

    /// Accesses client configuration.
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// Accesses the underlying client world view.
    pub fn world_view(&self) -> &ClientWorldView {
        &self.world_view
    }

    /// Mutable access to the underlying client world view.
    pub fn world_view_mut(&mut self) -> &mut ClientWorldView {
        &mut self.world_view
    }

    /// Returns a raw pointer to the pre-allocated render entities buffer in WASM linear memory.
    ///
    /// WebGL and JavaScript can read or upload this buffer directly via `WebAssembly.Memory`
    /// with zero serialization or allocation overhead.
    pub fn render_entities_ptr(&self) -> *const WebRenderEntity {
        self.render_entities.as_ptr()
    }

    /// Returns the count of valid extrapolated entities in the internal render buffer.
    pub fn render_entities_count(&self) -> usize {
        self.render_entities_count
    }

    /// Populates the internal render buffer with extrapolated entity transforms.
    ///
    /// Returns the number of visible entities populated.
    pub fn populate_render_entities_internal(&mut self, delta_seconds: f32) -> usize {
        let mut count = 0;
        for entity in self.world_view.iter() {
            if count >= MAX_WEB_RENDER_ENTITIES {
                break;
            }
            let transform = entity.extrapolate_transform(delta_seconds);
            self.render_entities[count] = WebRenderEntity {
                entity_id: entity.entity_id,
                x: transform.x,
                y: transform.y,
                z: transform.z,
                yaw_degrees: transform.yaw_deg,
                flags: transform.flags,
            };
            count += 1;
        }
        self.render_entities_count = count;
        count
    }

    /// Queues an internal client event in a ring buffer with drop-on-overflow semantics.
    fn push_event(&mut self, event: ClientEvent) {
        if self.event_count >= self.event_queue.len() {
            // Drop oldest on overflow
            self.event_head = (self.event_head + 1) % self.event_queue.len();
            self.event_count -= 1;
        }
        self.event_queue[self.event_tail] = Some(event);
        self.event_tail = (self.event_tail + 1) % self.event_queue.len();
        self.event_count += 1;
    }

    /// Polls the next queued event.
    pub fn poll_event(&mut self) -> Option<ClientEvent> {
        if self.event_count == 0 {
            return None;
        }
        let event = self.event_queue[self.event_head].take();
        self.event_head = (self.event_head + 1) % self.event_queue.len();
        self.event_count -= 1;
        event
    }

    /// Ingests a raw binary datagram with an explicit caller timestamp.
    ///
    /// Passing an explicit timestamp eliminates WASM-to-JS boundary crossings
    /// during high-frequency datagram ingestion.
    pub fn ingest_datagram_with_time(
        &mut self,
        packet: &[u8],
        timestamp: Instant,
    ) -> Result<usize, ClientError> {
        self.ingest_datagram_inner(packet, Some(timestamp))
    }

    /// Ingests a raw binary datagram received over WebTransport / WebSocket.
    ///
    /// Parses the packet header, extracts AoI transform updates, and updates
    /// the spatial world view with zero dynamic heap allocations.
    pub fn ingest_datagram(&mut self, packet: &[u8]) -> Result<usize, ClientError> {
        self.ingest_datagram_inner(packet, None)
    }

    fn ingest_datagram_inner(
        &mut self,
        packet: &[u8],
        timestamp: Option<Instant>,
    ) -> Result<usize, ClientError> {
        if packet.len() < HEADER_SIZE {
            return Err(ClientError::Net(NetError::TruncatedPacket {
                expected_len: HEADER_SIZE,
                actual_len: packet.len(),
            }));
        }

        let (header, hdr_len) = PacketHeader::read_from(packet)?;

        // Verify protocol version
        if header.version != PROTOCOL_VERSION {
            return Err(ClientError::Net(NetError::UnsupportedVersion {
                expected: PROTOCOL_VERSION,
                received: header.version,
            }));
        }

        self.recv_seq = header.sequence;
        let payload = packet.get(hdr_len..).unwrap_or(&[]);

        match header.packet_type {
            PacketType::StateUpdate => {
                let count = self.parse_state_updates(payload, timestamp);
                Ok(count)
            }
            PacketType::ReliableMessage => {
                if let Some(&action_opcode) = payload.first() {
                    self.push_event(ClientEvent::CombatAction {
                        source_id: header.sequence as u32,
                        target_id: 0,
                        action_type: action_opcode,
                        value: 0,
                    });
                }
                Ok(1)
            }
            PacketType::Disconnect => {
                self.world_view.clear();
                self.push_event(ClientEvent::Disconnected {
                    reason: "Server requested disconnect",
                });
                Ok(0)
            }
            _ => Ok(0),
        }
    }

    /// Parses 22-byte entity state updates from a state packet payload.
    fn parse_state_updates(&mut self, payload: &[u8], timestamp: Option<Instant>) -> usize {
        let record_size = 22;
        let mut offset = 0;
        let mut updated_count = 0;
        let now = timestamp.unwrap_or_else(Instant::now);

        while offset + record_size <= payload.len() {
            let chunk = &payload[offset..offset + record_size];
            let entity_id = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let cell_x = i16::from_be_bytes([chunk[4], chunk[5]]) as i32;
            let cell_z = i16::from_be_bytes([chunk[6], chunk[7]]) as i32;
            let cell_y = i16::from_be_bytes([chunk[8], chunk[9]]) as i32;
            let q_x = u16::from_be_bytes([chunk[10], chunk[11]]);
            let q_z = u16::from_be_bytes([chunk[12], chunk[13]]);
            let q_y = u16::from_be_bytes([chunk[14], chunk[15]]);
            let yaw = QuantizedYaw::from_byte(chunk[16]);
            let flags = chunk[17];
            let vx_raw = i16::from_be_bytes([chunk[18], chunk[19]]);
            let vz_raw = i16::from_be_bytes([chunk[20], chunk[21]]);

            let quant_coord = QuantizedCellCoord::new(q_x, q_y, q_z);
            let velocity = Vec3Fix {
                x: eidolon_core::fixed::Fixed64::from_i32(vx_raw as i32),
                y: eidolon_core::fixed::Fixed64::ZERO,
                z: eidolon_core::fixed::Fixed64::from_i32(vz_raw as i32),
            };

            let is_new = self.world_view.get_entity(entity_id).is_none();
            self.world_view.upsert_entity_with_time(
                entity_id,
                0,
                cell_x,
                cell_y,
                cell_z,
                quant_coord,
                yaw,
                flags,
                velocity,
                now,
            );

            let global_pos =
                QuantizedCellCoord::dequantize_to_global(cell_x, cell_y, cell_z, quant_coord);
            let (x, y, z) = global_pos.to_f64();

            if is_new {
                self.push_event(ClientEvent::EntitySpawned {
                    entity_id,
                    entity_type: 0,
                    x: x as f32,
                    y: y as f32,
                    z: z as f32,
                    yaw_deg: yaw.to_degrees() as f32,
                });
            } else {
                self.push_event(ClientEvent::EntityUpdated {
                    entity_id,
                    x: x as f32,
                    y: y as f32,
                    z: z as f32,
                    yaw_deg: yaw.to_degrees() as f32,
                    flags,
                });
            }

            offset += record_size;
            updated_count += 1;
        }

        updated_count
    }

    /// Populates an output slice with extrapolated render entities for WebGL/Canvas rendering.
    ///
    /// Returns the number of entities copied into `out`.
    pub fn populate_render_entities(
        &self,
        delta_seconds: f32,
        out: &mut [WebRenderEntity],
    ) -> usize {
        let mut count = 0;
        for entity in self.world_view.iter() {
            if count >= out.len() {
                break;
            }
            let transform = entity.extrapolate_transform(delta_seconds);
            out[count] = WebRenderEntity {
                entity_id: entity.entity_id,
                x: transform.x,
                y: transform.y,
                z: transform.z,
                yaw_degrees: transform.yaw_deg,
                flags: transform.flags,
            };
            count += 1;
        }
        count
    }

    /// Encodes a movement intent packet for transmission over WebTransport / WebSocket.
    pub fn encode_movement_intent(
        &mut self,
        vx: f32,
        vz: f32,
        yaw_deg: f32,
        flags: u8,
        out_buf: &mut [u8],
    ) -> Result<usize, ClientError> {
        let required = HEADER_SIZE + 13;
        if out_buf.len() < required {
            return Err(ClientError::Net(NetError::TruncatedPacket {
                expected_len: required,
                actual_len: out_buf.len(),
            }));
        }

        self.sequence = self.sequence.wrapping_add(1);

        let header = PacketHeader::new(
            ChannelType::UnreliableSequenced,
            PacketType::StateUpdate,
            self.sequence,
            self.recv_seq,
            self.ack_mask,
        );

        let hdr_len = header.write_to(out_buf)?;

        let mut idx = hdr_len;
        out_buf[idx..idx + 4].copy_from_slice(&vx.to_be_bytes());
        idx += 4;
        out_buf[idx..idx + 4].copy_from_slice(&vz.to_be_bytes());
        idx += 4;
        out_buf[idx..idx + 4].copy_from_slice(&yaw_deg.to_be_bytes());
        idx += 4;
        out_buf[idx] = flags;
        idx += 1;

        Ok(idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_client_lifecycle_and_extrapolation() {
        let mut client = WasmClient::new(ClientConfig::default());
        assert_eq!(client.world_view().count(), 0);

        // Manually insert an entity moving at 10 m/s in X
        client.world_view_mut().upsert_entity(
            101,
            0,
            0,
            0,
            0,
            QuantizedCellCoord::new(0, 0, 0),
            QuantizedYaw::EAST,
            1,
            Vec3Fix::from_f64(10.0, 0.0, 0.0),
        );
        assert_eq!(client.world_view().count(), 1);

        let mut render_buf = [WebRenderEntity::default(); 4];
        let count = client.populate_render_entities(0.166, &mut render_buf);
        assert_eq!(count, 1);
        assert_eq!(render_buf[0].entity_id, 101);

        // Extrapolated distance: 10 m/s * 0.166s = 1.66 meters
        assert!((render_buf[0].x - 1.66).abs() < 0.05);
        assert_eq!(render_buf[0].y, 0.0);
        assert_eq!(render_buf[0].z, 0.0);
        assert_eq!(render_buf[0].flags, 1);
    }

    #[test]
    fn test_wasm_movement_intent_encoding() {
        let mut client = WasmClient::new(ClientConfig::default());
        let mut buf = [0u8; 64];

        let len = client
            .encode_movement_intent(5.0, -2.5, 90.0, 0x05, &mut buf)
            .unwrap();
        assert_eq!(len, HEADER_SIZE + 13);

        let (header, hdr_len) = PacketHeader::read_from(&buf[..len]).unwrap();
        assert_eq!(header.packet_type, PacketType::StateUpdate);
        assert_eq!(header.channel, ChannelType::UnreliableSequenced);
        assert_eq!(header.sequence, 1);
        assert_eq!(hdr_len, HEADER_SIZE);

        let payload = &buf[hdr_len..len];
        let vx = f32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let vz = f32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
        let yaw = f32::from_be_bytes([payload[8], payload[9], payload[10], payload[11]]);
        let flags = payload[12];

        assert_eq!(vx, 5.0);
        assert_eq!(vz, -2.5);
        assert_eq!(yaw, 90.0);
        assert_eq!(flags, 0x05);
    }

    #[test]
    fn test_wasm_client_internal_render_buffer_and_ptr() {
        let mut client = WasmClient::new(ClientConfig::default());

        let ptr = client.render_entities_ptr();
        assert!(!ptr.is_null());
        assert_eq!(client.render_entities_count(), 0);

        // Add 3 entities
        for i in 1..=3 {
            client.world_view_mut().upsert_entity(
                i,
                0,
                0,
                0,
                0,
                QuantizedCellCoord::new(0, 0, 0),
                QuantizedYaw::from_degrees(0.0),
                0,
                Vec3Fix::from_f64((i * 5) as f64, 0.0, 0.0),
            );
        }

        let count = client.populate_render_entities_internal(0.1);
        assert_eq!(count, 3);
        assert_eq!(client.render_entities_count(), 3);
        let ids: Vec<u32> = (0..count)
            .map(|i| client.render_entities[i].entity_id)
            .collect();
        assert!(ids.contains(&1));
        assert!(ids.contains(&2));
        assert!(ids.contains(&3));
    }

    #[test]
    fn test_wasm_client_ingest_with_explicit_time() {
        let mut client = WasmClient::new(ClientConfig::default());
        let now = Instant::now();

        // Build a mock state packet with 1 entity update
        let header = PacketHeader::new(
            ChannelType::UnreliableSequenced,
            PacketType::StateUpdate,
            1,
            0,
            0,
        );
        let mut buf = [0u8; 64];
        let hdr_len = header.write_to(&mut buf).expect("write header");

        // Payload: 22 bytes entity state update
        let mut idx = hdr_len;
        // Entity ID 777
        buf[idx..idx + 4].copy_from_slice(&777u32.to_be_bytes());
        idx += 4;
        // Cell coords (0, 0, 0)
        idx += 6;
        // Quantized coords (0, 0, 0)
        idx += 6;
        // Yaw 0
        buf[idx] = 0;
        idx += 1;
        // Flags 1
        buf[idx] = 1;
        idx += 1;
        // Velocity (vx=10, vz=0)
        buf[idx..idx + 2].copy_from_slice(&10i16.to_be_bytes());
        idx += 2;
        buf[idx..idx + 2].copy_from_slice(&0i16.to_be_bytes());
        idx += 2;

        let res = client.ingest_datagram_with_time(&buf[..idx], now);
        assert_eq!(res.unwrap(), 1);
        assert_eq!(client.world_view().count(), 1);

        let entity = client.world_view().get_entity(777).expect("entity exists");
        assert_eq!(entity.last_update, now);
    }
}
