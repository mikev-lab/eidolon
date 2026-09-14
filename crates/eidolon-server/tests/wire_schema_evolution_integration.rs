//! Integration test suite for Phase 30: Zero-Copy Schema Evolution & Native Wire Bitstream Negotiation.
//!
//! Validates declarative schema definitions, handshake capability bitmask negotiation,
//! cross-version backward/forward wire compatibility, field-level delta XOR mask compression,
//! and graceful rejection of malicious/truncated bitstreams.

use eidolon_net::bitstream::{BitReader, BitWriter};
use eidolon_net::schema::{
    DeltaXorCompressor, FieldDescriptor, FieldType, FieldValue, SchemaEngine, SchemaError,
    SchemaNegotiator, WireSchema, MAX_SCHEMA_FIELDS,
};

#[test]
fn test_schema_handshake_capability_negotiation() {
    let mut server_schema = WireSchema::new(1, 2);

    // Version 1 fields
    server_schema
        .add_field(FieldDescriptor::new(0, "entity_id", FieldType::U32, 0, 1))
        .unwrap();
    server_schema
        .add_field(FieldDescriptor::new(
            1,
            "cell_x",
            FieldType::Quantized16,
            0,
            1,
        ))
        .unwrap();
    server_schema
        .add_field(FieldDescriptor::new(
            2,
            "cell_z",
            FieldType::Quantized16,
            0,
            1,
        ))
        .unwrap();
    server_schema
        .add_field(FieldDescriptor::new(
            3,
            "yaw",
            FieldType::QuantizedYaw,
            0,
            1,
        ))
        .unwrap();

    // Version 2 additions
    server_schema
        .add_field(FieldDescriptor::new(
            4,
            "vel_x",
            FieldType::Quantized16,
            0,
            2,
        ))
        .unwrap();
    server_schema
        .add_field(FieldDescriptor::new(
            5,
            "vel_z",
            FieldType::Quantized16,
            0,
            2,
        ))
        .unwrap();
    server_schema
        .add_field(FieldDescriptor::new(6, "flags", FieldType::Bits(4), 0, 2))
        .unwrap();

    let negotiator = SchemaNegotiator::new(server_schema, 1);

    // 1. Version 1 Client Handshake (supports fields 0..3, mask = 0b0000_1111 = 0x0F)
    let v1_client_mask = 0x0F;
    let session_v1 = negotiator.negotiate(1, v1_client_mask).unwrap();
    assert_eq!(session_v1.protocol_version, 1);
    assert_eq!(session_v1.active_field_mask, 0x0F);

    // 2. Version 2 Client Handshake (supports fields 0..6, mask = 0b0111_1111 = 0x7F)
    let v2_client_mask = 0x7F;
    let session_v2 = negotiator.negotiate(2, v2_client_mask).unwrap();
    assert_eq!(session_v2.protocol_version, 2);
    assert_eq!(session_v2.active_field_mask, 0x7F);

    // 3. Client version too old (< min_supported_version = 1)
    let err = negotiator.negotiate(0, 0x01).unwrap_err();
    assert!(matches!(
        err,
        SchemaError::VersionTooOld {
            client_version: 0,
            min_supported: 1
        }
    ));
}

#[test]
fn test_cross_version_wire_serialization_and_default_fallback() {
    let mut v2_schema = WireSchema::new(1, 2);
    v2_schema
        .add_field(FieldDescriptor::new(0, "entity_id", FieldType::U32, 0, 1))
        .unwrap();
    v2_schema
        .add_field(FieldDescriptor::new(1, "pos_x", FieldType::U16, 0, 1))
        .unwrap();
    v2_schema
        .add_field(FieldDescriptor::new(2, "yaw", FieldType::U8, 0, 1))
        .unwrap();
    // Introduced in v2 with default fallback value 100
    v2_schema
        .add_field(FieldDescriptor::new(3, "stamina", FieldType::U16, 100, 2))
        .unwrap();
    // Introduced in v2 with default fallback value 7
    v2_schema
        .add_field(FieldDescriptor::new(
            4,
            "status_flags",
            FieldType::Bits(4),
            7,
            2,
        ))
        .unwrap();

    let negotiator = SchemaNegotiator::new(v2_schema, 1);

    // A Version 1 client only knows fields 0..2
    let v1_client_mask = 0x07; // bits 0, 1, 2
    let session = negotiator.negotiate(1, v1_client_mask).unwrap();
    assert_eq!(session.active_field_mask, 0x07);

    // V1 Client serializes packet with fields 0, 1, 2
    let mut packet_buf = [0u8; 64];
    let mut writer = BitWriter::new(&mut packet_buf);
    let v1_values = [
        FieldValue::new(0, 42_000),
        FieldValue::new(1, 1_500),
        FieldValue::new(2, 180),
    ];
    SchemaEngine::serialize(
        &v2_schema,
        session.active_field_mask,
        &v1_values,
        &mut writer,
    )
    .unwrap();
    let written_bytes = writer.byte_len();

    // V2 Server receives packet and deserializes using the negotiated V1 session mask
    let mut reader = BitReader::new(&packet_buf[..written_bytes]);
    let mut decoded = [FieldValue::new(0, 0); 8];
    let decoded_count = SchemaEngine::deserialize(
        &v2_schema,
        session.active_field_mask,
        &mut reader,
        &mut decoded,
    )
    .unwrap();

    assert_eq!(decoded_count, 5); // Decoded all 5 fields in the v2 schema

    // Assert that fields 0..2 contain the wire data sent by the client
    assert_eq!(decoded[0], FieldValue::new(0, 42_000));
    assert_eq!(decoded[1], FieldValue::new(1, 1_500));
    assert_eq!(decoded[2], FieldValue::new(2, 180));

    // Assert that fields 3 and 4 were populated with their schema default values!
    assert_eq!(decoded[3], FieldValue::new(3, 100)); // stamina default
    assert_eq!(decoded[4], FieldValue::new(4, 7)); // status_flags default
}

#[test]
fn test_delta_xor_compression_and_zero_delta_suppression() {
    let active_mask = 0b0000_0111; // fields 0, 1, 2 active
    let baseline = [0u64; MAX_SCHEMA_FIELDS];

    // Scenario 1: Stationary entity (no fields changed)
    let current_stationary = baseline;
    let mut buffer = [0u8; 64];
    let (change_mask, len1) = {
        let mut writer = BitWriter::new(&mut buffer);
        let mask =
            DeltaXorCompressor::compress(&baseline, &current_stationary, active_mask, &mut writer)
                .unwrap();
        (mask, writer.byte_len())
    };

    assert_eq!(change_mask, 0); // No fields changed
    assert_eq!(len1, 4); // Only the 32-bit change mask was written!

    let mut reader = BitReader::new(&buffer[..len1]);
    let mut decompressed = [0u64; MAX_SCHEMA_FIELDS];
    let decoded_mask =
        DeltaXorCompressor::decompress(&baseline, &mut decompressed, &mut reader).unwrap();

    assert_eq!(decoded_mask, 0);
    assert_eq!(decompressed, baseline);

    // Scenario 2: Only 1 field changed (e.g. yaw at index 2 changed from 0 to 45)
    let mut current_moving = baseline;
    current_moving[2] = 45;

    let mut buffer2 = [0u8; 64];
    let (change_mask2, len2) = {
        let mut writer2 = BitWriter::new(&mut buffer2);
        let mask =
            DeltaXorCompressor::compress(&baseline, &current_moving, active_mask, &mut writer2)
                .unwrap();
        (mask, writer2.byte_len())
    };

    assert_eq!(change_mask2, 0b0000_0100); // Only bit 2 is set

    let mut reader2 = BitReader::new(&buffer2[..len2]);
    let mut decompressed2 = [0u64; MAX_SCHEMA_FIELDS];
    let decoded_mask2 =
        DeltaXorCompressor::decompress(&baseline, &mut decompressed2, &mut reader2).unwrap();

    assert_eq!(decoded_mask2, 0b0000_0100);
    assert_eq!(decompressed2[0], 0);
    assert_eq!(decompressed2[1], 0);
    assert_eq!(decompressed2[2], 45); // Accurately recovered!

    // Scenario 3: Multiple fields changed (indices 0 and 1)
    let mut current_multi = baseline;
    current_multi[0] = 0x1234_5678;
    current_multi[1] = 0xABCD;

    let mut buffer3 = [0u8; 64];
    let (change_mask3, len3) = {
        let mut writer3 = BitWriter::new(&mut buffer3);
        let mask =
            DeltaXorCompressor::compress(&baseline, &current_multi, active_mask, &mut writer3)
                .unwrap();
        (mask, writer3.byte_len())
    };

    assert_eq!(change_mask3, 0b0000_0011);

    let mut reader3 = BitReader::new(&buffer3[..len3]);
    let mut decompressed3 = [0u64; MAX_SCHEMA_FIELDS];
    DeltaXorCompressor::decompress(&baseline, &mut decompressed3, &mut reader3).unwrap();

    assert_eq!(decompressed3[0], 0x1234_5678);
    assert_eq!(decompressed3[1], 0xABCD);
}

#[test]
fn test_schema_malicious_truncated_bitstream_rejection() {
    let mut schema = WireSchema::new(1, 1);
    schema
        .add_field(FieldDescriptor::new(0, "val64", FieldType::U64, 0, 1))
        .unwrap();

    // 1. Truncated bitstream (only 2 bytes provided when 8 bytes / 64 bits required)
    let truncated_data = [0xAA, 0xBB];
    let mut reader = BitReader::new(&truncated_data);
    let mut out_values = [FieldValue::new(0, 0); 2];

    let result = SchemaEngine::deserialize(&schema, 0x01, &mut reader, &mut out_values);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), SchemaError::Bitstream(_)));

    // 2. Output buffer too small
    let full_data = [0u8; 8];
    let mut reader2 = BitReader::new(&full_data);
    let mut tiny_out = []; // 0 length

    let overflow_err = SchemaEngine::deserialize(&schema, 0x01, &mut reader2, &mut tiny_out);
    assert_eq!(overflow_err.unwrap_err(), SchemaError::OutputBufferTooSmall);

    // 3. Field ID out of range
    let invalid_field = FieldDescriptor::new(35, "bad", FieldType::U8, 0, 1);
    assert_eq!(
        schema.add_field(invalid_field).unwrap_err(),
        SchemaError::FieldIdOutOfRange(35)
    );
}
