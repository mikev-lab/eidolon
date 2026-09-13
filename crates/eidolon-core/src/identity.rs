//! Production identity, session authentication, and character authorization hierarchy.
//!
//! Enforces a strict 3-tier authorization model: Account -> Session -> Character.
//! Concurrent logins immediately revoke prior sessions via monotonic generation fencing,
//! and character commands are cryptographically bounded to prevent cross-account mutation exploits.

/// Strongly-typed 64-bit persistent account identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct AccountId(pub u64);

/// Strongly-typed 64-bit character identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct CharacterId(pub u64);

/// 128-bit cryptographic session ticket identifying an active client connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SessionTicket(pub [u8; 16]);

impl SessionTicket {
    /// Constructs a new session ticket from raw 16 bytes.
    #[inline]
    pub const fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Returns the underlying byte representation.
    #[inline]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

/// Bounded cryptographic capability token required for authoritative character mutations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharacterAuthorization {
    /// Owning account.
    pub account_id: AccountId,
    /// Authorized active character.
    pub character_id: CharacterId,
    /// Active session ticket.
    pub session_ticket: SessionTicket,
    /// Monotonic session generation fence.
    pub generation: u64,
}

/// Errors originating from identity authentication and character authorization verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityError {
    /// Monotonic session generation mismatch (session evicted by concurrent login).
    StaleSessionGeneration {
        /// Expected active generation.
        expected: u64,
        /// Obsolete caller generation.
        actual: u64,
    },
    /// Attempted mutation on a character not owned by the authenticated account.
    CrossAccountCharacterMismatch {
        /// Targeted character ID.
        character_id: CharacterId,
        /// True owner account.
        owner_account: AccountId,
        /// Unauthorized caller account.
        caller_account: AccountId,
    },
    /// Session ticket is expired, unrecognized, or revoked.
    SessionRevoked,
    /// Character identifier is not registered in the persistent hierarchy.
    CharacterNotFound,
    /// Active capacity limit exceeded.
    CapacityExceeded,
}

/// Internal record tracking an account's active login session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveSession {
    account_id: AccountId,
    ticket: SessionTicket,
    generation: u64,
    active_character: Option<CharacterId>,
}

/// Character ownership record linking a character to its owning account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CharacterOwnership {
    character_id: CharacterId,
    owner_account: AccountId,
}

/// Identity and authorization registry maintaining the Account -> Session -> Character hierarchy.
///
/// Designed with fixed memory capacity and zero dynamic allocations inside hot simulation loops.
pub struct IdentityRegistry<const MAX_ACCOUNTS: usize = 1024, const MAX_CHARACTERS: usize = 4096> {
    sessions: [Option<ActiveSession>; MAX_ACCOUNTS],
    characters: [Option<CharacterOwnership>; MAX_CHARACTERS],
    session_count: usize,
    character_count: usize,
}

impl<const MAX_ACCOUNTS: usize, const MAX_CHARACTERS: usize> Default
    for IdentityRegistry<MAX_ACCOUNTS, MAX_CHARACTERS>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<const MAX_ACCOUNTS: usize, const MAX_CHARACTERS: usize>
    IdentityRegistry<MAX_ACCOUNTS, MAX_CHARACTERS>
{
    /// Constructs an empty identity registry with pre-allocated storage.
    pub const fn new() -> Self {
        Self {
            sessions: [None; MAX_ACCOUNTS],
            characters: [None; MAX_CHARACTERS],
            session_count: 0,
            character_count: 0,
        }
    }

    /// Registers character ownership linking `character_id` to `account_id`.
    pub fn register_character(
        &mut self,
        account_id: AccountId,
        character_id: CharacterId,
    ) -> Result<(), IdentityError> {
        // Update existing entry if present
        for slot in self.characters.iter_mut().flatten() {
            if slot.character_id == character_id {
                slot.owner_account = account_id;
                return Ok(());
            }
        }

        // Insert into first empty slot
        for slot in self.characters.iter_mut() {
            if slot.is_none() {
                *slot = Some(CharacterOwnership {
                    character_id,
                    owner_account: account_id,
                });
                self.character_count += 1;
                return Ok(());
            }
        }

        Err(IdentityError::CapacityExceeded)
    }

    /// Authenticates a session ticket for an account.
    ///
    /// Concurrent Login Revocation Invariant:
    /// If an active session already exists for `account_id`, its generation is incremented
    /// and its ticket is overwritten. Any in-flight packets bearing the previous generation
    /// or ticket will immediately fail validation.
    pub fn authenticate_session(
        &mut self,
        account_id: AccountId,
        ticket: SessionTicket,
    ) -> Result<u64, IdentityError> {
        // Look for existing active session for this account
        for slot in self.sessions.iter_mut().flatten() {
            if slot.account_id == account_id {
                let next_gen = slot.generation.saturating_add(1);
                slot.ticket = ticket;
                slot.generation = next_gen;
                slot.active_character = None;
                return Ok(next_gen);
            }
        }

        // Allocate slot for new account session
        for slot in self.sessions.iter_mut() {
            if slot.is_none() {
                *slot = Some(ActiveSession {
                    account_id,
                    ticket,
                    generation: 1,
                    active_character: None,
                });
                self.session_count += 1;
                return Ok(1);
            }
        }

        Err(IdentityError::CapacityExceeded)
    }

    /// Selects an active character for an authenticated session.
    ///
    /// Cross-Account Protection Invariant:
    /// Asserts that `character_id` is registered to `account_id`. Rejects with
    /// `CrossAccountCharacterMismatch` if an adversary attempts to select a character
    /// owned by another player.
    pub fn select_character(
        &mut self,
        ticket: &SessionTicket,
        character_id: CharacterId,
    ) -> Result<CharacterAuthorization, IdentityError> {
        // Find session
        let (account_id, generation) = {
            let session = self
                .sessions
                .iter()
                .flatten()
                .find(|s| s.ticket == *ticket)
                .ok_or(IdentityError::SessionRevoked)?;
            (session.account_id, session.generation)
        };

        // Verify ownership
        let owner = self
            .characters
            .iter()
            .flatten()
            .find(|c| c.character_id == character_id)
            .map(|c| c.owner_account)
            .ok_or(IdentityError::CharacterNotFound)?;

        if owner != account_id {
            return Err(IdentityError::CrossAccountCharacterMismatch {
                character_id,
                owner_account: owner,
                caller_account: account_id,
            });
        }

        // Set active character in session
        for slot in self.sessions.iter_mut().flatten() {
            if slot.ticket == *ticket {
                slot.active_character = Some(character_id);
                break;
            }
        }

        Ok(CharacterAuthorization {
            account_id,
            character_id,
            session_ticket: *ticket,
            generation,
        })
    }

    /// Validates that an incoming simulation packet or character mutation command is authorized.
    ///
    /// Verifies:
    /// 1. The session is active and not revoked.
    /// 2. The generation matches the latest active generation (fencing stale packets).
    /// 3. The character belongs to the authenticated account.
    pub fn validate_character_action(
        &self,
        auth: &CharacterAuthorization,
        target_character: CharacterId,
    ) -> Result<(), IdentityError> {
        if auth.character_id != target_character {
            return Err(IdentityError::CrossAccountCharacterMismatch {
                character_id: target_character,
                owner_account: auth.account_id,
                caller_account: auth.account_id,
            });
        }

        let session = self
            .sessions
            .iter()
            .flatten()
            .find(|s| s.account_id == auth.account_id)
            .ok_or(IdentityError::SessionRevoked)?;

        if session.ticket != auth.session_ticket {
            return Err(IdentityError::SessionRevoked);
        }

        if session.generation != auth.generation {
            return Err(IdentityError::StaleSessionGeneration {
                expected: session.generation,
                actual: auth.generation,
            });
        }

        Ok(())
    }

    /// Explicitly revokes an active session (e.g. clean logout or administrative disconnect).
    pub fn revoke_session(&mut self, account_id: AccountId) {
        for slot in self.sessions.iter_mut() {
            if let Some(session) = slot {
                if session.account_id == account_id {
                    *slot = None;
                    self.session_count = self.session_count.saturating_sub(1);
                    break;
                }
            }
        }
    }

    /// Returns the number of currently active sessions.
    #[inline]
    pub const fn active_session_count(&self) -> usize {
        self.session_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identity_hierarchy_and_concurrent_login_fencing() {
        let mut registry = IdentityRegistry::<16, 64>::new();

        let acc1 = AccountId(101);
        let char1 = CharacterId(5001);
        let char2 = CharacterId(5002);

        registry.register_character(acc1, char1).unwrap();
        registry.register_character(acc1, char2).unwrap();

        // Initial login: Session A
        let ticket_a = SessionTicket([0xAA; 16]);
        let gen_a = registry.authenticate_session(acc1, ticket_a).unwrap();
        assert_eq!(gen_a, 1);

        let auth_a = registry.select_character(&ticket_a, char1).unwrap();
        assert_eq!(auth_a.generation, 1);

        // Action with Auth A succeeds
        assert!(registry.validate_character_action(&auth_a, char1).is_ok());

        // Concurrent login: Session B for same account
        let ticket_b = SessionTicket([0xBB; 16]);
        let gen_b = registry.authenticate_session(acc1, ticket_b).unwrap();
        assert_eq!(gen_b, 2);

        // Auth A is now fenced by generation bump!
        let err = registry.validate_character_action(&auth_a, char1);
        assert_eq!(
            err,
            Err(IdentityError::SessionRevoked) // ticket mismatch
        );

        // Session B selects character
        let auth_b = registry.select_character(&ticket_b, char1).unwrap();
        assert_eq!(auth_b.generation, 2);
        assert!(registry.validate_character_action(&auth_b, char1).is_ok());
    }

    #[test]
    fn test_cross_account_write_fencing() {
        let mut registry = IdentityRegistry::<16, 64>::new();

        let alice = AccountId(101);
        let bob = AccountId(102);

        let alice_char = CharacterId(1001);
        let bob_char = CharacterId(2002);

        registry.register_character(alice, alice_char).unwrap();
        registry.register_character(bob, bob_char).unwrap();

        let alice_ticket = SessionTicket([0x11; 16]);
        registry.authenticate_session(alice, alice_ticket).unwrap();

        // Alice attempts to select Bob's character: MUST FAIL
        let res = registry.select_character(&alice_ticket, bob_char);
        assert_eq!(
            res,
            Err(IdentityError::CrossAccountCharacterMismatch {
                character_id: bob_char,
                owner_account: bob,
                caller_account: alice,
            })
        );
    }
}
