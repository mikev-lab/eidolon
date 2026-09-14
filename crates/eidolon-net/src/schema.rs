//! Zero-copy declarative wire schema evolution and handshake bitstream negotiation.
//!
//! Provides forward- and backward-compatible packet serialization, capability bitmask
//! negotiation between mixed client/server versions, and field-level delta XOR mask
//! compression with zero dynamic heap allocations and zero third-party dependencies.

#![deny(unsafe_code)]

use crate::bitstream::{BitReader, BitWriter};
use crate::error::BitstreamError;
use std::fmt;

/// Maximum number of distinct fields allowed in a single declarative schema.
pub const MAX_SCHEMA_FIELDS: usize = 32;

/// Declarative data type specification for schema fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    /// 8-bit unsigned integer.
    U8,
    /// 16-bit unsigned integer.
    U16,
    /// 32-bit unsigned integer.
    U32,
    /// 64-bit unsigned integer.
    U64,
    /// Custom bit-width integer (1 to 64 bits).
    Bits(u8),
    /// Variable-length LEB128 integer.
    Varint,
    /// 16-bit cell-relative quantized coordinate.
    Quantized16,
    /// 8-bit quantized yaw orientation angle.
    QuantizedYaw,
    /// Delta XOR compressed field relative to baseline.
    DeltaXor,
}

impl FieldType {
    /// Returns the static bit-width if known at compile time, or None if variable.
    #[inline]
    pub fn static_bit_width(&self) -> Option<usize> {
        match *self {
            Self::U8 | Self::QuantizedYaw => Some(8),
            Self::U16 | Self::Quantized16 => Some(16),
            Self::U32 => Some(32),
            Self::U64 => Some(64),
            Self::Bits(w) => Some(w as usize),
            Self::Varint | Self::DeltaXor => None,
        }
    }
}

/// Declarative specification of a single packet field in the wire schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldDescriptor {
    /// Unique 0-indexed field identifier (0 to 31).
    pub field_id: u8,
    /// Human-readable diagnostic name.
    pub name: &'static str,
    /// Wire data type encoding.
    pub field_type: FieldType,
    /// Default fallback value used when field is absent in an older protocol version.
    pub default_value: u64,
    /// Protocol version where this field was introduced.
    pub introduced_in_version: u16,
    /// Protocol version where this field was deprecated (if any).
    pub deprecated_in_version: Option<u16>,
}

impl FieldDescriptor {
    /// Creates a new field descriptor introduced in a specific version.
    pub const fn new(
        field_id: u8,
        name: &'static str,
        field_type: FieldType,
        default_value: u64,
        introduced_in_version: u16,
    ) -> Self {
        Self {
            field_id,
            name,
            field_type,
            default_value,
            introduced_in_version,
            deprecated_in_version: None,
        }
    }

    /// Sets the deprecation version for this field.
    pub const fn with_deprecation(mut self, deprecated_in_version: u16) -> Self {
        self.deprecated_in_version = Some(deprecated_in_version);
        self
    }

    /// Returns true if this field is active and supported in the given protocol version.
    #[inline]
    pub fn is_active_in_version(&self, version: u16) -> bool {
        if version < self.introduced_in_version {
            return false;
        }
        if let Some(deprecated) = self.deprecated_in_version {
            if version >= deprecated {
                return false;
            }
        }
        true
    }
}

/// Declarative wire schema containing an ordered table of fields.
#[derive(Debug, Clone, Copy)]
pub struct WireSchema {
    /// Authoritative schema identifier (e.g. packet type opcode).
    pub schema_id: u16,
    /// Engine schema revision version.
    pub version: u16,
    /// Pre-allocated array of field descriptors.
    pub fields: [Option<FieldDescriptor>; MAX_SCHEMA_FIELDS],
    /// Total count of registered fields.
    pub field_count: usize,
}

impl WireSchema {
    /// Creates an empty schema with specified ID and version.
    pub const fn new(schema_id: u16, version: u16) -> Self {
        Self {
            schema_id,
            version,
            fields: [None; MAX_SCHEMA_FIELDS],
            field_count: 0,
        }
    }

    /// Registers a field descriptor into the schema.
    pub fn add_field(&mut self, descriptor: FieldDescriptor) -> Result<(), SchemaError> {
        let idx = descriptor.field_id as usize;
        if idx >= MAX_SCHEMA_FIELDS {
            return Err(SchemaError::FieldIdOutOfRange(descriptor.field_id));
        }
        if self.fields[idx].is_none() {
            self.field_count += 1;
        }
        self.fields[idx] = Some(descriptor);
        Ok(())
    }

    /// Looks up a field descriptor by field ID.
    #[inline]
    pub fn get_field(&self, field_id: u8) -> Option<&FieldDescriptor> {
        let idx = field_id as usize;
        if idx < MAX_SCHEMA_FIELDS {
            self.fields[idx].as_ref()
        } else {
            None
        }
    }

    /// Calculates the 32-bit field presence bitmask for a given protocol version.
    pub fn compute_field_mask(&self, protocol_version: u16) -> u32 {
        let mut mask = 0u32;
        for field in self.fields.iter().flatten() {
            if field.is_active_in_version(protocol_version) {
                mask |= 1 << (field.field_id as usize);
            }
        }
        mask
    }
}

/// Negotiated capability state established during client/server connection handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NegotiatedSession {
    /// Mutually agreed active protocol version.
    pub protocol_version: u16,
    /// Negotiated active field bitmask (intersection of client and server capabilities).
    pub active_field_mask: u32,
}

/// Negotiator matching client wire capabilities against server schema definitions.
#[derive(Debug, Clone, Copy)]
pub struct SchemaNegotiator {
    server_schema: WireSchema,
    min_supported_version: u16,
}

impl SchemaNegotiator {
    /// Creates a new schema negotiator for a server schema.
    pub const fn new(server_schema: WireSchema, min_supported_version: u16) -> Self {
        Self {
            server_schema,
            min_supported_version,
        }
    }

    /// Negotiates an active session given client version and capability bitmask.
    pub fn negotiate(
        &self,
        client_version: u16,
        client_field_mask: u32,
    ) -> Result<NegotiatedSession, SchemaError> {
        if client_version < self.min_supported_version {
            return Err(SchemaError::VersionTooOld {
                client_version,
                min_supported: self.min_supported_version,
            });
        }

        let agreed_version = client_version.min(self.server_schema.version);
        let server_mask = self.server_schema.compute_field_mask(agreed_version);

        // Bitwise intersection: both client and server must support the field on the wire
        let active_field_mask = server_mask & client_field_mask;

        Ok(NegotiatedSession {
            protocol_version: agreed_version,
            active_field_mask,
        })
    }
}

/// Concrete field instance containing raw value representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldValue {
    /// Target field ID.
    pub field_id: u8,
    /// Unsigned 64-bit value container.
    pub value: u64,
}

impl FieldValue {
    /// Creates a new field value.
    #[inline]
    pub const fn new(field_id: u8, value: u64) -> Self {
        Self { field_id, value }
    }
}

/// Field-level delta XOR compressor for stationary and linear transform streams.
pub struct DeltaXorCompressor;

impl DeltaXorCompressor {
    /// Compresses a set of values against baseline values, returning the changed field mask.
    pub fn compress(
        baseline: &[u64; MAX_SCHEMA_FIELDS],
        current: &[u64; MAX_SCHEMA_FIELDS],
        active_mask: u32,
        writer: &mut BitWriter<'_>,
    ) -> Result<u32, SchemaError> {
        let mut change_mask = 0u32;

        // 1. Identify which active fields actually changed
        for i in 0..MAX_SCHEMA_FIELDS {
            if (active_mask & (1 << i)) != 0 && current[i] != baseline[i] {
                change_mask |= 1 << i;
            }
        }

        // 2. Write 32-bit change mask
        writer
            .write_bits(change_mask as u64, 32)
            .map_err(SchemaError::Bitstream)?;

        // 3. For each changed field, write variable-length XOR delta
        for i in 0..MAX_SCHEMA_FIELDS {
            if (change_mask & (1 << i)) != 0 {
                let delta = current[i] ^ baseline[i];
                writer.write_varint(delta).map_err(SchemaError::Bitstream)?;
            }
        }

        Ok(change_mask)
    }

    /// Decompresses delta XOR stream against baseline values.
    pub fn decompress(
        baseline: &[u64; MAX_SCHEMA_FIELDS],
        out_current: &mut [u64; MAX_SCHEMA_FIELDS],
        reader: &mut BitReader<'_>,
    ) -> Result<u32, SchemaError> {
        // Start from baseline
        *out_current = *baseline;

        // 1. Read 32-bit change mask
        let change_mask = reader.read_bits(32).map_err(SchemaError::Bitstream)? as u32;

        // 2. Read and apply XOR deltas for changed fields
        for i in 0..MAX_SCHEMA_FIELDS {
            if (change_mask & (1 << i)) != 0 {
                let delta = reader.read_varint().map_err(SchemaError::Bitstream)?;
                out_current[i] = baseline[i] ^ delta;
            }
        }

        Ok(change_mask)
    }
}

/// Zero-copy wire serializer and deserializer for schema-evolved packets.
pub struct SchemaEngine;

impl SchemaEngine {
    /// Serializes field values into a bitstream using negotiated schema capabilities.
    pub fn serialize(
        schema: &WireSchema,
        active_mask: u32,
        values: &[FieldValue],
        writer: &mut BitWriter<'_>,
    ) -> Result<(), SchemaError> {
        for field in schema.fields.iter().flatten() {
            let field_id = field.field_id;
            let bit = 1 << (field_id as usize);

            // Skip fields not negotiated in active session
            if (active_mask & bit) == 0 {
                continue;
            }

            // Find provided value or fallback to default
            let raw_val = values
                .iter()
                .find(|v| v.field_id == field_id)
                .map(|v| v.value)
                .unwrap_or(field.default_value);

            // Encode according to field type
            match field.field_type {
                FieldType::U8 | FieldType::QuantizedYaw => {
                    writer
                        .write_bits(raw_val, 8)
                        .map_err(SchemaError::Bitstream)?;
                }
                FieldType::U16 | FieldType::Quantized16 => {
                    writer
                        .write_bits(raw_val, 16)
                        .map_err(SchemaError::Bitstream)?;
                }
                FieldType::U32 => {
                    writer
                        .write_bits(raw_val, 32)
                        .map_err(SchemaError::Bitstream)?;
                }
                FieldType::U64 => {
                    writer
                        .write_bits(raw_val, 64)
                        .map_err(SchemaError::Bitstream)?;
                }
                FieldType::Bits(width) => {
                    writer
                        .write_bits(raw_val, width as usize)
                        .map_err(SchemaError::Bitstream)?;
                }
                FieldType::Varint | FieldType::DeltaXor => {
                    writer
                        .write_varint(raw_val)
                        .map_err(SchemaError::Bitstream)?;
                }
            }
        }

        Ok(())
    }

    /// Deserializes fields from a bitstream using negotiated schema capabilities.
    ///
    /// Fields omitted by older versions are populated using the schema's default value.
    pub fn deserialize(
        schema: &WireSchema,
        active_mask: u32,
        reader: &mut BitReader<'_>,
        out_values: &mut [FieldValue],
    ) -> Result<usize, SchemaError> {
        let mut decoded_count = 0;

        for field in schema.fields.iter().flatten() {
            if decoded_count >= out_values.len() {
                return Err(SchemaError::OutputBufferTooSmall);
            }

            let field_id = field.field_id;
            let bit = 1 << (field_id as usize);

            let value = if (active_mask & bit) != 0 {
                // Read from wire
                match field.field_type {
                    FieldType::U8 | FieldType::QuantizedYaw => {
                        reader.read_bits(8).map_err(SchemaError::Bitstream)?
                    }
                    FieldType::U16 | FieldType::Quantized16 => {
                        reader.read_bits(16).map_err(SchemaError::Bitstream)?
                    }
                    FieldType::U32 => reader.read_bits(32).map_err(SchemaError::Bitstream)?,
                    FieldType::U64 => reader.read_bits(64).map_err(SchemaError::Bitstream)?,
                    FieldType::Bits(width) => reader
                        .read_bits(width as usize)
                        .map_err(SchemaError::Bitstream)?,
                    FieldType::Varint | FieldType::DeltaXor => {
                        reader.read_varint().map_err(SchemaError::Bitstream)?
                    }
                }
            } else {
                // Absent in this protocol version: use schema default value
                field.default_value
            };

            out_values[decoded_count] = FieldValue::new(field_id, value);
            decoded_count += 1;
        }

        Ok(decoded_count)
    }
}

/// Errors originating from schema registration, negotiation, or wire serialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaError {
    /// Underlying bitstream read or write failure.
    Bitstream(BitstreamError),
    /// Field ID exceeded the fixed maximum schema capacity.
    FieldIdOutOfRange(u8),
    /// Client protocol version is too old and no longer supported.
    VersionTooOld {
        /// Client requested protocol version.
        client_version: u16,
        /// Minimum server supported version.
        min_supported: u16,
    },
    /// Provided output buffer was too small to hold all decoded fields.
    OutputBufferTooSmall,
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bitstream(err) => write!(f, "Schema bitstream error: {err}"),
            Self::FieldIdOutOfRange(id) => {
                write!(f, "Field ID {id} exceeds maximum schema capacity")
            }
            Self::VersionTooOld {
                client_version,
                min_supported,
            } => write!(
                f,
                "Client version {client_version} is below minimum supported version {min_supported}"
            ),
            Self::OutputBufferTooSmall => {
                write!(f, "Output slice too small for decoded schema fields")
            }
        }
    }
}

impl std::error::Error for SchemaError {}

impl From<BitstreamError> for SchemaError {
    fn from(err: BitstreamError) -> Self {
        Self::Bitstream(err)
    }
}
