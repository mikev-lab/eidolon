//! Register-width bitstream reader and writer primitives.
//!
//! Provides zero-copy, zero-heap-allocation bit-level serialization and deserialization
//! with safe bounds checks, variable-length integer (varint) encodings, and byte alignment.

use eidolon_core::delta::{DeltaTier, DeltaTransform};
use eidolon_core::quant::{QuantizedCellCoord, QuantizedYaw};

use crate::error::BitstreamError;

/// Bounded bitstream writer operating directly on caller-provided byte slices.
#[derive(Debug)]
pub struct BitWriter<'a> {
    buffer: &'a mut [u8],
    bit_offset: usize,
}

impl<'a> BitWriter<'a> {
    /// Creates a new `BitWriter` wrapping a mutable byte slice.
    ///
    /// Clears the initial buffer slice up to its capacity to guarantee zeroed bits.
    #[inline]
    pub fn new(buffer: &'a mut [u8]) -> Self {
        buffer.fill(0);
        Self {
            buffer,
            bit_offset: 0,
        }
    }

    /// Creates a `BitWriter` wrapping a mutable byte slice without zeroing existing memory.
    #[inline]
    pub fn from_existing(buffer: &'a mut [u8], bit_offset: usize) -> Self {
        Self { buffer, bit_offset }
    }

    /// Returns the current bit position within the stream.
    #[inline]
    pub fn bit_offset(&self) -> usize {
        self.bit_offset
    }

    /// Returns the number of fully or partially written bytes.
    #[inline]
    pub fn byte_len(&self) -> usize {
        self.bit_offset.div_ceil(8)
    }

    /// Returns the total capacity of the underlying byte slice in bytes.
    #[inline]
    pub fn buffer_capacity(&self) -> usize {
        self.buffer.len()
    }

    /// Returns the total capacity of the underlying buffer in bits.
    #[inline]
    pub fn total_bits(&self) -> usize {
        self.buffer.len().saturating_mul(8)
    }

    /// Returns the number of remaining bits that can be written.
    #[inline]
    pub fn remaining_bits(&self) -> usize {
        self.total_bits().saturating_sub(self.bit_offset)
    }

    /// Returns an immutable subslice of the written bytes.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        let len = self.byte_len();
        self.buffer.get(..len).unwrap_or(&[])
    }

    /// Writes a single boolean bit (1 if true, 0 if false).
    #[inline]
    pub fn write_bit(&mut self, value: bool) -> Result<(), BitstreamError> {
        let byte_idx = self.bit_offset / 8;
        let bit_idx = self.bit_offset % 8;

        if let Some(byte) = self.buffer.get_mut(byte_idx) {
            let mask = 1u8 << (7 - bit_idx);
            if value {
                *byte |= mask;
            } else {
                *byte &= !mask;
            }
            self.bit_offset += 1;
            Ok(())
        } else {
            Err(BitstreamError::BufferOverflow {
                required_bits: 1,
                available_bits: 0,
            })
        }
    }

    /// Writes arbitrary unsigned bits (1 to 64 bits).
    pub fn write_bits(&mut self, value: u64, num_bits: usize) -> Result<(), BitstreamError> {
        if num_bits == 0 {
            return Ok(());
        }
        if num_bits > 64 {
            return Err(BitstreamError::InvalidBitWidth(num_bits));
        }

        let remaining = self.remaining_bits();
        if num_bits > remaining {
            return Err(BitstreamError::BufferOverflow {
                required_bits: num_bits,
                available_bits: remaining,
            });
        }

        // Fast path for byte-aligned writes
        if self.bit_offset.is_multiple_of(8) {
            let byte_idx = self.bit_offset / 8;
            if num_bits == 8 {
                if let Some(byte) = self.buffer.get_mut(byte_idx) {
                    *byte = value as u8;
                    self.bit_offset += 8;
                    return Ok(());
                }
            } else if num_bits == 16 {
                if let Some(bytes) = self.buffer.get_mut(byte_idx..byte_idx + 2) {
                    bytes.copy_from_slice(&(value as u16).to_be_bytes());
                    self.bit_offset += 16;
                    return Ok(());
                }
            } else if num_bits == 32 {
                if let Some(bytes) = self.buffer.get_mut(byte_idx..byte_idx + 4) {
                    bytes.copy_from_slice(&(value as u32).to_be_bytes());
                    self.bit_offset += 32;
                    return Ok(());
                }
            } else if num_bits == 64 {
                if let Some(bytes) = self.buffer.get_mut(byte_idx..byte_idx + 8) {
                    bytes.copy_from_slice(&value.to_be_bytes());
                    self.bit_offset += 64;
                    return Ok(());
                }
            }
        }

        let mut rem_bits = num_bits;
        let cur_val = if num_bits == 64 {
            value
        } else {
            value & ((1u64 << num_bits) - 1)
        };

        while rem_bits > 0 {
            let byte_idx = self.bit_offset / 8;
            let bit_idx = self.bit_offset % 8;
            let avail = 8 - bit_idx;
            let take = rem_bits.min(avail);
            let shift_src = rem_bits - take;
            let chunk = ((cur_val >> shift_src) as u8) & (((1u16 << take) - 1) as u8);
            let shift_dst = avail - take;
            let mask = (((1u16 << take) - 1) as u8) << shift_dst;
            if let Some(byte) = self.buffer.get_mut(byte_idx) {
                *byte = (*byte & !mask) | (chunk << shift_dst);
            }
            self.bit_offset += take;
            rem_bits -= take;
        }

        Ok(())
    }

    /// Writes a single 8-bit unsigned integer.
    #[inline]
    pub fn write_u8(&mut self, val: u8) -> Result<(), BitstreamError> {
        if self.bit_offset.is_multiple_of(8) {
            let byte_idx = self.bit_offset / 8;
            if let Some(byte) = self.buffer.get_mut(byte_idx) {
                *byte = val;
                self.bit_offset += 8;
                return Ok(());
            }
        }
        self.write_bits(val as u64, 8)
    }

    /// Writes a 16-bit unsigned integer.
    #[inline]
    pub fn write_u16(&mut self, val: u16) -> Result<(), BitstreamError> {
        if self.bit_offset.is_multiple_of(8) {
            let byte_idx = self.bit_offset / 8;
            if let Some(bytes) = self.buffer.get_mut(byte_idx..byte_idx + 2) {
                bytes.copy_from_slice(&val.to_be_bytes());
                self.bit_offset += 16;
                return Ok(());
            }
        }
        self.write_bits(val as u64, 16)
    }

    /// Writes a 32-bit unsigned integer.
    #[inline]
    pub fn write_u32(&mut self, val: u32) -> Result<(), BitstreamError> {
        if self.bit_offset.is_multiple_of(8) {
            let byte_idx = self.bit_offset / 8;
            if let Some(bytes) = self.buffer.get_mut(byte_idx..byte_idx + 4) {
                bytes.copy_from_slice(&val.to_be_bytes());
                self.bit_offset += 32;
                return Ok(());
            }
        }
        self.write_bits(val as u64, 32)
    }

    /// Writes a 64-bit unsigned integer.
    #[inline]
    pub fn write_u64(&mut self, val: u64) -> Result<(), BitstreamError> {
        if self.bit_offset.is_multiple_of(8) {
            let byte_idx = self.bit_offset / 8;
            if let Some(bytes) = self.buffer.get_mut(byte_idx..byte_idx + 8) {
                bytes.copy_from_slice(&val.to_be_bytes());
                self.bit_offset += 64;
                return Ok(());
            }
        }
        self.write_bits(val, 64)
    }

    /// Writes a variable-length 64-bit integer using 7-bit chunks with MSB continuation.
    pub fn write_varint(&mut self, mut value: u64) -> Result<usize, BitstreamError> {
        let mut bytes_written = 0;
        loop {
            let mut byte = (value & 0x7F) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            self.write_u8(byte)?;
            bytes_written += 1;
            if value == 0 {
                break;
            }
        }
        Ok(bytes_written)
    }

    /// Writes raw bytes into the stream.
    pub fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), BitstreamError> {
        let bits_needed = bytes.len().saturating_mul(8);
        if bits_needed > self.remaining_bits() {
            return Err(BitstreamError::BufferOverflow {
                required_bits: bits_needed,
                available_bits: self.remaining_bits(),
            });
        }

        // Fast path when byte-aligned
        if self.bit_offset.is_multiple_of(8) {
            let start = self.bit_offset / 8;
            let end = start + bytes.len();
            if let Some(dest) = self.buffer.get_mut(start..end) {
                dest.copy_from_slice(bytes);
                self.bit_offset += bits_needed;
                return Ok(());
            }
        }

        for &b in bytes {
            self.write_u8(b)?;
        }
        Ok(())
    }

    /// Pads the bitstream to the next byte boundary with zero bits.
    #[inline]
    pub fn flush_byte_alignment(&mut self) -> Result<(), BitstreamError> {
        let rem = self.bit_offset % 8;
        if rem != 0 {
            let padding = 8 - rem;
            self.write_bits(0, padding)?;
        }
        Ok(())
    }

    /// Writes an adaptive variable-bit delta transform update into the bitstream.
    pub fn write_delta_transform(&mut self, delta: &DeltaTransform) -> Result<(), BitstreamError> {
        match *delta {
            DeltaTransform::Stationary => self.write_bits(DeltaTier::Stationary.tag() as u64, 2),
            DeltaTransform::Small { dx, dz, dy, dyaw } => {
                self.write_bits(DeltaTier::Small.tag() as u64, 2)?;
                let u_dx = (dx + 8) as u64 & 0x0F;
                let u_dz = (dz + 8) as u64 & 0x0F;
                let u_dy = (dy + 4) as u64 & 0x07;
                let u_dyaw = (dyaw + 4) as u64 & 0x07;
                self.write_bits(u_dx, 4)?;
                self.write_bits(u_dz, 4)?;
                self.write_bits(u_dy, 3)?;
                self.write_bits(u_dyaw, 3)
            }
            DeltaTransform::Medium { dx, dz, dy, yaw } => {
                self.write_bits(DeltaTier::Medium.tag() as u64, 2)?;
                let u_dx = dx as u8 as u64;
                let u_dz = dz as u8 as u64;
                let u_dy = (dy + 32) as u64 & 0x3F;
                let u_yaw = yaw.as_byte() as u64;
                self.write_bits(u_dy, 6)?;
                self.write_bits(u_dx, 8)?;
                self.write_bits(u_dz, 8)?;
                self.write_bits(u_yaw, 8)
            }
            DeltaTransform::Full { coord, yaw, flags } => {
                self.write_bits(DeltaTier::Full.tag() as u64, 2)?;
                let full_7b = coord.pack_with_yaw_and_flags(yaw, flags);
                self.write_bytes(&full_7b)
            }
        }
    }
}

/// Bounded bitstream reader operating directly on caller-provided immutable byte slices.
#[derive(Debug)]
pub struct BitReader<'a> {
    buffer: &'a [u8],
    bit_offset: usize,
}

impl<'a> BitReader<'a> {
    /// Creates a new `BitReader` wrapping an immutable byte slice.
    #[inline]
    pub fn new(buffer: &'a [u8]) -> Self {
        Self {
            buffer,
            bit_offset: 0,
        }
    }

    /// Returns current bit offset within the slice.
    #[inline]
    pub fn bit_offset(&self) -> usize {
        self.bit_offset
    }

    /// Returns the total number of bits in the underlying buffer.
    #[inline]
    pub fn total_bits(&self) -> usize {
        self.buffer.len().saturating_mul(8)
    }

    /// Returns the number of remaining bits available to read.
    #[inline]
    pub fn remaining_bits(&self) -> usize {
        self.total_bits().saturating_sub(self.bit_offset)
    }

    /// Reads a single boolean bit.
    #[inline]
    pub fn read_bit(&mut self) -> Result<bool, BitstreamError> {
        let byte_idx = self.bit_offset / 8;
        let bit_idx = self.bit_offset % 8;

        if let Some(&byte) = self.buffer.get(byte_idx) {
            let mask = 1u8 << (7 - bit_idx);
            let is_set = (byte & mask) != 0;
            self.bit_offset += 1;
            Ok(is_set)
        } else {
            Err(BitstreamError::UnexpectedEof {
                requested_bits: 1,
                remaining_bits: 0,
            })
        }
    }

    /// Reads arbitrary unsigned bits (1 to 64 bits).
    pub fn read_bits(&mut self, num_bits: usize) -> Result<u64, BitstreamError> {
        if num_bits == 0 {
            return Ok(0);
        }
        if num_bits > 64 {
            return Err(BitstreamError::InvalidBitWidth(num_bits));
        }

        let remaining = self.remaining_bits();
        if num_bits > remaining {
            return Err(BitstreamError::UnexpectedEof {
                requested_bits: num_bits,
                remaining_bits: remaining,
            });
        }

        // Fast paths for byte-aligned reads
        if self.bit_offset.is_multiple_of(8) {
            let byte_idx = self.bit_offset / 8;
            if num_bits == 8 {
                if let Some(&byte) = self.buffer.get(byte_idx) {
                    self.bit_offset += 8;
                    return Ok(byte as u64);
                }
            } else if num_bits == 16 {
                if let Some(bytes) = self.buffer.get(byte_idx..byte_idx + 2) {
                    self.bit_offset += 16;
                    return Ok(u16::from_be_bytes([bytes[0], bytes[1]]) as u64);
                }
            } else if num_bits == 32 {
                if let Some(bytes) = self.buffer.get(byte_idx..byte_idx + 4) {
                    self.bit_offset += 32;
                    return Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as u64);
                }
            } else if num_bits == 64 {
                if let Some(bytes) = self.buffer.get(byte_idx..byte_idx + 8) {
                    self.bit_offset += 64;
                    return Ok(u64::from_be_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6],
                        bytes[7],
                    ]));
                }
            }
        }

        let mut value = 0u64;
        let mut rem_bits = num_bits;
        while rem_bits > 0 {
            let byte_idx = self.bit_offset / 8;
            let bit_idx = self.bit_offset % 8;
            let avail = 8 - bit_idx;
            let take = rem_bits.min(avail);
            let shift = avail - take;
            let mask = (((1u16 << take) - 1) as u8) << shift;
            if let Some(&byte) = self.buffer.get(byte_idx) {
                let chunk = ((byte & mask) >> shift) as u64;
                value = (value << take) | chunk;
            }
            self.bit_offset += take;
            rem_bits -= take;
        }

        Ok(value)
    }

    /// Reads an 8-bit unsigned integer.
    #[inline]
    pub fn read_u8(&mut self) -> Result<u8, BitstreamError> {
        if self.bit_offset.is_multiple_of(8) {
            let byte_idx = self.bit_offset / 8;
            if let Some(&byte) = self.buffer.get(byte_idx) {
                self.bit_offset += 8;
                return Ok(byte);
            }
        }
        self.read_bits(8).map(|v| v as u8)
    }

    /// Reads a 16-bit unsigned integer.
    #[inline]
    pub fn read_u16(&mut self) -> Result<u16, BitstreamError> {
        if self.bit_offset.is_multiple_of(8) {
            let byte_idx = self.bit_offset / 8;
            if let Some(bytes) = self.buffer.get(byte_idx..byte_idx + 2) {
                self.bit_offset += 16;
                return Ok(u16::from_be_bytes([bytes[0], bytes[1]]));
            }
        }
        self.read_bits(16).map(|v| v as u16)
    }

    /// Reads a 32-bit unsigned integer.
    #[inline]
    pub fn read_u32(&mut self) -> Result<u32, BitstreamError> {
        if self.bit_offset.is_multiple_of(8) {
            let byte_idx = self.bit_offset / 8;
            if let Some(bytes) = self.buffer.get(byte_idx..byte_idx + 4) {
                self.bit_offset += 32;
                return Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]));
            }
        }
        self.read_bits(32).map(|v| v as u32)
    }

    /// Reads a 64-bit unsigned integer.
    #[inline]
    pub fn read_u64(&mut self) -> Result<u64, BitstreamError> {
        if self.bit_offset.is_multiple_of(8) {
            let byte_idx = self.bit_offset / 8;
            if let Some(bytes) = self.buffer.get(byte_idx..byte_idx + 8) {
                self.bit_offset += 64;
                return Ok(u64::from_be_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
                ]));
            }
        }
        self.read_bits(64)
    }

    /// Reads a variable-length 64-bit integer.
    ///
    /// Enforces a maximum of 10 bytes to prevent unbounded continuation attacks.
    pub fn read_varint(&mut self) -> Result<u64, BitstreamError> {
        let mut result = 0u64;
        let mut shift = 0;

        for i in 0..10 {
            let byte = self.read_u8()?;
            let val = (byte & 0x7F) as u64;

            // Check for 64-bit integer overflow on the 10th byte
            if i == 9 && (val & 0x7E) != 0 {
                return Err(BitstreamError::InvalidVarint);
            }

            result |= val << shift;
            if (byte & 0x80) == 0 {
                return Ok(result);
            }
            shift += 7;
        }

        Err(BitstreamError::InvalidVarint)
    }

    /// Reads raw bytes into the provided slice.
    pub fn read_bytes(&mut self, out: &mut [u8]) -> Result<(), BitstreamError> {
        let bits_needed = out.len().saturating_mul(8);
        if bits_needed > self.remaining_bits() {
            return Err(BitstreamError::UnexpectedEof {
                requested_bits: bits_needed,
                remaining_bits: self.remaining_bits(),
            });
        }

        // Fast path when byte-aligned
        if self.bit_offset.is_multiple_of(8) {
            let start = self.bit_offset / 8;
            let end = start + out.len();
            if let Some(src) = self.buffer.get(start..end) {
                out.copy_from_slice(src);
                self.bit_offset += bits_needed;
                return Ok(());
            }
        }

        for b in out.iter_mut() {
            *b = self.read_u8()?;
        }
        Ok(())
    }

    /// Reads an adaptive variable-bit delta transform update from the bitstream.
    pub fn read_delta_transform(&mut self) -> Result<DeltaTransform, BitstreamError> {
        let tag = self.read_bits(2)? as u8;
        let tier = DeltaTier::from_tag(tag);
        match tier {
            DeltaTier::Stationary => Ok(DeltaTransform::Stationary),
            DeltaTier::Small => {
                let u_dx = self.read_bits(4)? as i8;
                let u_dz = self.read_bits(4)? as i8;
                let u_dy = self.read_bits(3)? as i8;
                let u_dyaw = self.read_bits(3)? as i8;
                Ok(DeltaTransform::Small {
                    dx: u_dx - 8,
                    dz: u_dz - 8,
                    dy: u_dy - 4,
                    dyaw: u_dyaw - 4,
                })
            }
            DeltaTier::Medium => {
                let u_dy = self.read_bits(6)? as i8;
                let dx = self.read_bits(8)? as u8 as i8;
                let dz = self.read_bits(8)? as u8 as i8;
                let yaw_byte = self.read_bits(8)? as u8;
                Ok(DeltaTransform::Medium {
                    dx,
                    dz,
                    dy: u_dy - 32,
                    yaw: QuantizedYaw::from_byte(yaw_byte),
                })
            }
            DeltaTier::Full => {
                let mut buf_7b = [0u8; 7];
                self.read_bytes(&mut buf_7b)?;
                let (coord, yaw, flags) = QuantizedCellCoord::unpack_with_yaw_and_flags(buf_7b);
                Ok(DeltaTransform::Full { coord, yaw, flags })
            }
        }
    }

    /// Aligns read cursor to the next byte boundary.
    #[inline]
    pub fn align_to_byte(&mut self) {
        let rem = self.bit_offset % 8;
        if rem != 0 {
            self.bit_offset += 8 - rem;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bit_writer_reader_single_bits() {
        let mut buf = [0u8; 2];
        let mut writer = BitWriter::new(&mut buf);
        writer.write_bit(true).expect("write 1");
        writer.write_bit(false).expect("write 0");
        writer.write_bit(true).expect("write 1");
        writer.write_bit(true).expect("write 1");

        let mut reader = BitReader::new(&buf);
        assert!(reader.read_bit().expect("read 1"));
        assert!(!reader.read_bit().expect("read 0"));
        assert!(reader.read_bit().expect("read 1"));
        assert!(reader.read_bit().expect("read 1"));
    }

    #[test]
    fn test_arbitrary_bit_widths() {
        let mut buf = [0u8; 8];
        let mut writer = BitWriter::new(&mut buf);
        writer.write_bits(0b101, 3).expect("write 3 bits");
        writer.write_bits(0b11001, 5).expect("write 5 bits");
        writer.write_bits(0xABCD, 16).expect("write 16 bits");
        writer.write_bits(42, 12).expect("write 12 bits");

        let mut reader = BitReader::new(&buf);
        assert_eq!(reader.read_bits(3).expect("read 3 bits"), 0b101);
        assert_eq!(reader.read_bits(5).expect("read 5 bits"), 0b11001);
        assert_eq!(reader.read_bits(16).expect("read 16 bits"), 0xABCD);
        assert_eq!(reader.read_bits(12).expect("read 12 bits"), 42);
    }

    #[test]
    fn test_varint_roundtrip() {
        let test_values = [
            0u64,
            1,
            127,
            128,
            255,
            300,
            16383,
            16384,
            u32::MAX as u64,
            u64::MAX - 1,
            u64::MAX,
        ];

        for &val in &test_values {
            let mut buf = [0u8; 16];
            let mut writer = BitWriter::new(&mut buf);
            let written = writer.write_varint(val).expect("write varint");
            assert!(written <= 10);

            let mut reader = BitReader::new(writer.as_bytes());
            let decoded = reader.read_varint().expect("read varint");
            assert_eq!(val, decoded);
        }
    }

    #[test]
    fn test_overflow_and_eof_protection() {
        let mut buf = [0u8; 1];
        let mut writer = BitWriter::new(&mut buf);
        assert!(writer.write_bits(0xFF, 8).is_ok());
        assert!(writer.write_bit(true).is_err());

        let mut reader = BitReader::new(&buf);
        assert!(reader.read_bits(8).is_ok());
        assert!(reader.read_bit().is_err());
    }

    #[test]
    fn test_varint_malicious_continuation_attack() {
        // 11 continuation bytes
        let hostile_buf = [0xFFu8; 11];
        let mut reader = BitReader::new(&hostile_buf);
        let result = reader.read_varint();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), BitstreamError::InvalidVarint);
    }

    #[test]
    fn test_delta_transform_bitstream_roundtrip() {
        let base_coord = QuantizedCellCoord::new(2000, 400, 3000);
        let base_yaw = QuantizedYaw::from_degrees(90.0);

        let deltas = [
            DeltaTransform::Stationary,
            DeltaTransform::Small {
                dx: 3,
                dz: -2,
                dy: 1,
                dyaw: -1,
            },
            DeltaTransform::Medium {
                dx: -45,
                dz: 60,
                dy: -10,
                yaw: QuantizedYaw::from_degrees(180.0),
            },
            DeltaTransform::Full {
                coord: base_coord,
                yaw: base_yaw,
                flags: 0x05,
            },
        ];

        let mut buf = [0u8; 64];
        let mut writer = BitWriter::new(&mut buf);

        for delta in &deltas {
            writer.write_delta_transform(delta).expect("write delta");
        }

        let mut reader = BitReader::new(writer.as_bytes());
        for delta in &deltas {
            let decoded = reader.read_delta_transform().expect("read delta");
            assert_eq!(*delta, decoded);
        }
    }
}
