//! Cold-state account hibernation and sub-5ms profile hydration.
//!
//! Enables live-service and gacha game backends to scale to zero idle cost
//! by persisting dormant player inventories and pity counters as compact (<1 KB) binary snapshots.

use eidolon_core::fixed::Vec3Fix;

use crate::error::WorldError;
use crate::transaction::AccountState;
use crate::zone::WorldZone;

/// Protocol magic header for hibernated account snapshots ('E', 'I', 'H', 'B' -> 0x45, 0x49, 0x48, 0x42).
pub const HIBERNATION_MAGIC: [u8; 4] = [0x45, 0x49, 0x48, 0x42];

/// Current binary snapshot schema version.
pub const HIBERNATION_SCHEMA_VERSION: u16 = 1;

/// Maximum number of characters stored in the compact player roster snapshot.
pub const MAX_ROSTER_SIZE: usize = 32;

/// Fixed-size header for a cold-state account snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HibernationHeader {
    /// Protocol magic identifier.
    pub magic: [u8; 4],
    /// Schema version.
    pub version: u16,
    /// Account identifier.
    pub account_id: u64,
    /// Last recorded login timestamp (seconds since UNIX epoch).
    pub last_active_epoch_sec: u64,
    /// Payload length in bytes excluding header.
    pub payload_byte_len: u32,
    /// Adler-32 checksum of the payload data.
    pub checksum: u32,
}

/// Gacha pity counters and guaranteed banner pull states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PityState {
    /// Pity counter on standard permanent banner.
    pub standard_banner_pity: u16,
    /// Pity counter on limited event banner.
    pub limited_banner_pity: u16,
    /// Flag indicating next 5-star character is guaranteed to be the banner rate-up character.
    pub is_guaranteed_rate_up: bool,
    /// Consecutive 4-star pity counter.
    pub consecutive_4star_pity: u8,
}

/// Compact character state representation within the player roster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CharacterRecord {
    /// Unique character blueprint identifier.
    pub character_id: u32,
    /// Character progression level (1 to 90).
    pub level: u16,
    /// Ascension rank (0 to 6).
    pub ascension_tier: u8,
    /// Duplicate constellation unlocked rank (0 to 6).
    pub constellation: u8,
}

/// Active player profile data model supporting cold-state serialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerProfile {
    /// Unique account identifier.
    pub account_id: u64,
    /// Overall account / player rank.
    pub player_level: u32,
    /// Premium gacha currency balance.
    pub premium_currency: u64,
    /// Standard farmable game currency balance.
    pub free_currency: u64,
    /// Gacha pity and guarantee counters.
    pub pity: PityState,
    /// Owned character collection.
    pub roster: [Option<CharacterRecord>; MAX_ROSTER_SIZE],
    /// Active number of unlocked characters.
    pub character_count: usize,
}

impl PlayerProfile {
    /// Creates a newly initialized player profile.
    pub fn new(account_id: u64) -> Self {
        Self {
            account_id,
            player_level: 1,
            premium_currency: 0,
            free_currency: 0,
            pity: PityState::default(),
            roster: [None; MAX_ROSTER_SIZE],
            character_count: 0,
        }
    }

    /// Adds or updates a character record in the player's collection.
    pub fn add_character(&mut self, char_record: CharacterRecord) -> Result<(), WorldError> {
        // Check if character is already owned; if so, update
        for slot in self.roster.iter_mut().flatten() {
            if slot.character_id == char_record.character_id {
                *slot = char_record;
                return Ok(());
            }
        }

        // Insert into first empty slot
        let slot = self
            .roster
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(WorldError::InvalidSnapshot("Roster capacity reached"))?;

        *slot = Some(char_record);
        self.character_count += 1;
        Ok(())
    }

    /// Serializes the player profile into a compact binary snapshot buffer.
    ///
    /// The entire snapshot fits within 512 bytes with zero heap allocations.
    pub fn serialize_snapshot(&self, epoch_sec: u64, out: &mut [u8]) -> Result<usize, WorldError> {
        if out.len() < 512 {
            return Err(WorldError::InvalidSnapshot("Buffer too small"));
        }

        // Payload begins at byte 32 (reserving 32 bytes for the header)
        let mut cursor = 32;

        // Account level (4 bytes)
        out[cursor..cursor + 4].copy_from_slice(&self.player_level.to_be_bytes());
        cursor += 4;

        // Currencies (8 + 8 = 16 bytes)
        out[cursor..cursor + 8].copy_from_slice(&self.premium_currency.to_be_bytes());
        cursor += 8;
        out[cursor..cursor + 8].copy_from_slice(&self.free_currency.to_be_bytes());
        cursor += 8;

        // Pity State (2 + 2 + 1 + 1 = 6 bytes)
        out[cursor..cursor + 2].copy_from_slice(&self.pity.standard_banner_pity.to_be_bytes());
        cursor += 2;
        out[cursor..cursor + 2].copy_from_slice(&self.pity.limited_banner_pity.to_be_bytes());
        cursor += 2;
        out[cursor] = if self.pity.is_guaranteed_rate_up {
            1
        } else {
            0
        };
        cursor += 1;
        out[cursor] = self.pity.consecutive_4star_pity;
        cursor += 1;

        // Character Roster (1 byte count + count * 8 bytes per character)
        out[cursor] = self.character_count as u8;
        cursor += 1;

        for char_opt in self.roster.iter().flatten() {
            out[cursor..cursor + 4].copy_from_slice(&char_opt.character_id.to_be_bytes());
            cursor += 4;
            out[cursor..cursor + 2].copy_from_slice(&char_opt.level.to_be_bytes());
            cursor += 2;
            out[cursor] = char_opt.ascension_tier;
            cursor += 1;
            out[cursor] = char_opt.constellation;
            cursor += 1;
        }

        let payload_len = (cursor - 32) as u32;
        let payload_slice = &out[32..cursor];
        let checksum = compute_adler32(payload_slice);

        // Serialize 32-byte header
        out[0..4].copy_from_slice(&HIBERNATION_MAGIC);
        out[4..6].copy_from_slice(&HIBERNATION_SCHEMA_VERSION.to_be_bytes());
        out[6..14].copy_from_slice(&self.account_id.to_be_bytes());
        out[14..22].copy_from_slice(&epoch_sec.to_be_bytes());
        out[22..26].copy_from_slice(&payload_len.to_be_bytes());
        out[26..30].copy_from_slice(&checksum.to_be_bytes());
        out[30..32].fill(0); // Reserved padding

        Ok(cursor)
    }

    /// Deserializes and hydrates a player profile from a cold binary snapshot.
    ///
    /// Executes in sub-millisecond time with zero dynamic heap allocations.
    pub fn deserialize_snapshot(slice: &[u8]) -> Result<Self, WorldError> {
        if slice.len() < 32 {
            return Err(WorldError::InvalidSnapshot("Snapshot too short"));
        }

        let magic = [slice[0], slice[1], slice[2], slice[3]];
        if magic != HIBERNATION_MAGIC {
            return Err(WorldError::InvalidSnapshot("Invalid magic bytes"));
        }

        let version = u16::from_be_bytes([slice[4], slice[5]]);
        if version != HIBERNATION_SCHEMA_VERSION {
            return Err(WorldError::InvalidSnapshot("Unsupported schema version"));
        }

        let account_id = u64::from_be_bytes([
            slice[6], slice[7], slice[8], slice[9], slice[10], slice[11], slice[12], slice[13],
        ]);

        let payload_len = u32::from_be_bytes([slice[22], slice[23], slice[24], slice[25]]) as usize;
        let expected_checksum = u32::from_be_bytes([slice[26], slice[27], slice[28], slice[29]]);

        if slice.len() < 32 + payload_len {
            return Err(WorldError::InvalidSnapshot("Truncated payload"));
        }

        let payload_slice = &slice[32..32 + payload_len];
        let actual_checksum = compute_adler32(payload_slice);
        if actual_checksum != expected_checksum {
            return Err(WorldError::SnapshotCorrupted);
        }

        // Unpack payload
        let mut cursor = 32;

        let player_level = u32::from_be_bytes([
            slice[cursor],
            slice[cursor + 1],
            slice[cursor + 2],
            slice[cursor + 3],
        ]);
        cursor += 4;

        let premium_currency = u64::from_be_bytes([
            slice[cursor],
            slice[cursor + 1],
            slice[cursor + 2],
            slice[cursor + 3],
            slice[cursor + 4],
            slice[cursor + 5],
            slice[cursor + 6],
            slice[cursor + 7],
        ]);
        cursor += 8;

        let free_currency = u64::from_be_bytes([
            slice[cursor],
            slice[cursor + 1],
            slice[cursor + 2],
            slice[cursor + 3],
            slice[cursor + 4],
            slice[cursor + 5],
            slice[cursor + 6],
            slice[cursor + 7],
        ]);
        cursor += 8;

        let standard_banner_pity = u16::from_be_bytes([slice[cursor], slice[cursor + 1]]);
        cursor += 2;
        let limited_banner_pity = u16::from_be_bytes([slice[cursor], slice[cursor + 1]]);
        cursor += 2;
        let is_guaranteed_rate_up = slice[cursor] == 1;
        cursor += 1;
        let consecutive_4star_pity = slice[cursor];
        cursor += 1;

        let pity = PityState {
            standard_banner_pity,
            limited_banner_pity,
            is_guaranteed_rate_up,
            consecutive_4star_pity,
        };

        let char_count = slice[cursor] as usize;
        cursor += 1;

        let mut roster = [None; MAX_ROSTER_SIZE];
        for slot in roster.iter_mut().take(char_count) {
            let character_id = u32::from_be_bytes([
                slice[cursor],
                slice[cursor + 1],
                slice[cursor + 2],
                slice[cursor + 3],
            ]);
            cursor += 4;
            let level = u16::from_be_bytes([slice[cursor], slice[cursor + 1]]);
            cursor += 2;
            let ascension_tier = slice[cursor];
            cursor += 1;
            let constellation = slice[cursor];
            cursor += 1;

            *slot = Some(CharacterRecord {
                character_id,
                level,
                ascension_tier,
                constellation,
            });
        }

        Ok(Self {
            account_id,
            player_level,
            premium_currency,
            free_currency,
            pity,
            roster,
            character_count: char_count.min(MAX_ROSTER_SIZE),
        })
    }
}

/// Hydrates a cold player profile directly into an active zone's spatial grid and transaction state.
///
/// Spawns the player's primary character entity at the target position, returning the initialized account state.
pub fn hydrate_player_into_zone(
    profile: &PlayerProfile,
    zone: &mut WorldZone,
    spawn_pos: Vec3Fix,
    entity_id: u32,
) -> Result<AccountState, WorldError> {
    zone.insert_entity(entity_id, spawn_pos)?;

    let mut account_state = AccountState::new(profile.account_id);
    account_state.premium_currency = profile.premium_currency;
    account_state.free_currency = profile.free_currency;

    Ok(account_state)
}

/// Computes a deterministic Adler-32 checksum without external dependencies.
#[inline]
pub fn compute_adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_hibernation_roundtrip() {
        let mut profile = PlayerProfile::new(9876543210);
        profile.player_level = 60;
        profile.premium_currency = 16000;
        profile.free_currency = 5000000;
        profile.pity.limited_banner_pity = 75;
        profile.pity.is_guaranteed_rate_up = true;

        profile
            .add_character(CharacterRecord {
                character_id: 1001,
                level: 90,
                ascension_tier: 6,
                constellation: 2,
            })
            .expect("add char 1");

        profile
            .add_character(CharacterRecord {
                character_id: 2002,
                level: 80,
                ascension_tier: 5,
                constellation: 0,
            })
            .expect("add char 2");

        let mut buffer = [0u8; 512];
        let written_len = profile
            .serialize_snapshot(1700000000, &mut buffer)
            .expect("serialize snapshot");

        // Footprint must remain well under 512 bytes (<1 KB)
        assert!(written_len < 256);

        let hydrated =
            PlayerProfile::deserialize_snapshot(&buffer[..written_len]).expect("hydrate profile");

        assert_eq!(profile.account_id, hydrated.account_id);
        assert_eq!(profile.player_level, hydrated.player_level);
        assert_eq!(profile.premium_currency, hydrated.premium_currency);
        assert_eq!(profile.free_currency, hydrated.free_currency);
        assert_eq!(profile.pity, hydrated.pity);
        assert_eq!(profile.character_count, hydrated.character_count);
        assert_eq!(profile.roster, hydrated.roster);
    }

    #[test]
    fn test_corrupted_snapshot_checksum_failure() {
        let mut profile = PlayerProfile::new(12345);
        profile.player_level = 10;
        let mut buffer = [0u8; 512];
        let len = profile
            .serialize_snapshot(1700000000, &mut buffer)
            .expect("serialize");

        // Corrupt a byte in the payload
        buffer[35] ^= 0xFF;

        let result = PlayerProfile::deserialize_snapshot(&buffer[..len]);
        assert_eq!(result.unwrap_err(), WorldError::SnapshotCorrupted);
    }

    #[test]
    fn test_hydrate_player_into_zone_spatial_and_wallet() {
        use crate::error::ZoneId;
        use crate::zone::{SeamAxis, ZoneBounds};
        use eidolon_core::fixed::Fixed64;

        let bounds = ZoneBounds::new(
            Fixed64::from_i32(0),
            Fixed64::from_i32(100),
            Fixed64::from_i32(0),
            Fixed64::from_i32(100),
            SeamAxis::EastWest,
            Fixed64::from_i32(84),
            Fixed64::from_i32(100),
        );
        let mut zone = WorldZone::new(ZoneId(1), bounds, None, true, 64);

        let mut profile = PlayerProfile::new(777);
        profile.premium_currency = 5000;
        profile.free_currency = 25000;

        let spawn_pos = Vec3Fix::new(
            Fixed64::from_i32(25),
            Fixed64::from_i32(0),
            Fixed64::from_i32(25),
        );
        let account_state = hydrate_player_into_zone(&profile, &mut zone, spawn_pos, 42)
            .expect("Hydration must succeed");

        assert_eq!(account_state.account_id, 777);
        assert_eq!(account_state.premium_currency, 5000);
        assert_eq!(account_state.free_currency, 25000);
        assert_eq!(zone.spatial_grid.get_position(42), Some(spawn_pos));
    }
}
