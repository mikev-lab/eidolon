//! Milestone 14.2 Integration Test Suite: Cryptographic CAVP & RFC 4231 Validation.
//!
//! Validates first-principles native cryptographic primitives against official NIST CAVP
//! and RFC 4231 test vectors, verifies constant-time equality protections, and asserts
//! the pluggable CryptoProvider abstraction.

use eidolon_net::crypto::{
    constant_time_eq, hmac_sha256, sha256, CryptoProvider, NativeCryptoProvider,
};

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[test]
fn test_milestone_14_2_nist_cavp_sha256_standard_vectors() {
    // 1. Empty message
    let empty_hash = sha256(b"");
    assert_eq!(
        to_hex(&empty_hash),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );

    // 2. Short single-block message ("abc")
    let abc_hash = sha256(b"abc");
    assert_eq!(
        to_hex(&abc_hash),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );

    // 3. 56-byte message testing exact 2-block padding boundary
    let msg_56 = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
    let hash_56 = sha256(msg_56);
    assert_eq!(
        to_hex(&hash_56),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );

    // 4. NIST CAVP LongMsg: 1,000,000 repetitions of ASCII character "a"
    let one_million_as = vec![b'a'; 1_000_000];
    let long_hash = sha256(&one_million_as);
    assert_eq!(
        to_hex(&long_hash),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
}

#[test]
fn test_milestone_14_2_rfc4231_hmac_sha256_full_suite() {
    // Case 1
    let key1 = [0x0bu8; 20];
    let data1 = b"Hi There";
    let mac1 = hmac_sha256(&key1, data1);
    assert_eq!(
        to_hex(&mac1),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );

    // Case 2: key shorter than HMAC output length
    let key2 = b"Jefe";
    let data2 = b"what do ya want for nothing?";
    let mac2 = hmac_sha256(key2, data2);
    assert_eq!(
        to_hex(&mac2),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );

    // Case 3: combined length of key and data exceeds 64-byte block size
    let key3 = [0xaau8; 20];
    let data3 = [0xddu8; 50];
    let mac3 = hmac_sha256(&key3, &data3);
    assert_eq!(
        to_hex(&mac3),
        "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe"
    );

    // Case 4: 25-byte key
    let key4: Vec<u8> = (1..=25).collect();
    let data4 = [0xcdu8; 50];
    let mac4 = hmac_sha256(&key4, &data4);
    assert_eq!(
        to_hex(&mac4),
        "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b"
    );

    // Case 5: 128-bit truncated output
    let key5 = [0x0cu8; 20];
    let data5 = b"Test With Truncation";
    let mac5 = hmac_sha256(&key5, data5);
    assert_eq!(to_hex(&mac5[..16]), "a3b6167473100ee06e0c796c2955552b");

    // Case 6: key larger than 64-byte block size (131 bytes)
    let key6 = [0xaau8; 131];
    let data6 = b"Test Using Larger Than Block-Size Key - Hash Key First";
    let mac6 = hmac_sha256(&key6, data6);
    assert_eq!(
        to_hex(&mac6),
        "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
    );

    // Case 7: key larger than 64 bytes and data larger than block size
    let key7 = [0xaau8; 131];
    let data7 = b"This is a test using a larger than block-size key and a larger than block-size data. The key needs to be hashed before being used by the HMAC algorithm.";
    let mac7 = hmac_sha256(&key7, data7);
    assert_eq!(
        to_hex(&mac7),
        "9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2"
    );
}

#[test]
fn test_milestone_14_2_constant_time_equality_side_channel_defense() {
    let original = [0x5Au8; 32];
    assert!(constant_time_eq(&original, &original));

    // Test that every single byte position from 0 to 31 rejects equality
    for i in 0..32 {
        let mut corrupted = original;
        corrupted[i] ^= 0x01; // 1-bit flip
        assert!(
            !constant_time_eq(&original, &corrupted),
            "Failed to reject 1-bit mismatch at byte index {i}"
        );

        corrupted[i] ^= 0xFF; // Full byte flip
        assert!(
            !constant_time_eq(&original, &corrupted),
            "Failed to reject full byte mismatch at byte index {i}"
        );
    }
}

/// Simulated HSM / Hardware Crypto Provider demonstrating seamless trait pluggability.
struct MockHsmCryptoProvider {
    operation_count: std::sync::atomic::AtomicU64,
}

impl MockHsmCryptoProvider {
    fn new() -> Self {
        Self {
            operation_count: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn ops(&self) -> u64 {
        self.operation_count
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl CryptoProvider for MockHsmCryptoProvider {
    fn sha256(&self, data: &[u8]) -> [u8; 32] {
        self.operation_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        sha256(data)
    }

    fn hmac_sha256(&self, key: &[u8], data: &[u8]) -> [u8; 32] {
        self.operation_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        hmac_sha256(key, data)
    }

    fn constant_time_eq(&self, a: &[u8; 32], b: &[u8; 32]) -> bool {
        self.operation_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        constant_time_eq(a, b)
    }
}

#[test]
fn test_milestone_14_2_crypto_provider_pluggability() {
    // Verify default native provider
    let native = NativeCryptoProvider;
    let native_digest = native.sha256(b"hello world");
    assert_eq!(
        to_hex(&native_digest),
        "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
    );

    // Verify mock HSM provider satisfying CryptoProvider trait
    let hsm = MockHsmCryptoProvider::new();
    let provider: &dyn CryptoProvider = &hsm;

    let d = provider.sha256(b"hello world");
    assert_eq!(d, native_digest);

    let key = b"secret-key";
    let mac = provider.hmac_sha256(key, b"payload");
    assert!(provider.constant_time_eq(&mac, &mac));

    assert_eq!(hsm.ops(), 3);
}
