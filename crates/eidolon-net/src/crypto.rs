//! Pure first-principles cryptographic primitives: SHA-256 and HMAC-SHA256.
//!
//! Engineered from first principles in native Rust without external crates, complying with
//! NIST FIPS 180-4 and RFC 2104. All routines operate with zero heap allocations and
//! constant-time verification to prevent timing side-channel attacks.

/// Initial SHA-256 hash values (first 32 bits of the fractional parts of square roots of first 8 primes).
const H_INIT: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// SHA-256 round constants (first 32 bits of the fractional parts of cube roots of first 64 primes).
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

#[inline(always)]
fn rotr(x: u32, n: u32) -> u32 {
    x.rotate_right(n)
}

#[inline(always)]
fn ch(x: u32, y: u32, z: u32) -> u32 {
    (x & y) ^ (!x & z)
}

#[inline(always)]
fn maj(x: u32, y: u32, z: u32) -> u32 {
    (x & y) ^ (x & z) ^ (y & z)
}

#[inline(always)]
fn sigma0(x: u32) -> u32 {
    rotr(x, 2) ^ rotr(x, 13) ^ rotr(x, 22)
}

#[inline(always)]
fn sigma1(x: u32) -> u32 {
    rotr(x, 6) ^ rotr(x, 11) ^ rotr(x, 25)
}

#[inline(always)]
fn gamma0(x: u32) -> u32 {
    rotr(x, 7) ^ rotr(x, 18) ^ (x >> 3)
}

#[inline(always)]
fn gamma1(x: u32) -> u32 {
    rotr(x, 17) ^ rotr(x, 19) ^ (x >> 10)
}

/// Computes the SHA-256 hash of a byte slice according to FIPS 180-4.
///
/// Operates entirely in fixed stack buffers with zero heap allocations.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut state = H_INIT;
    let mut w = [0u32; 64];

    let total_len = data.len();
    let bit_len = (total_len as u64).wrapping_mul(8);

    // Process full 64-byte chunks
    let mut offset = 0;
    while offset + 64 <= total_len {
        let chunk = match data.get(offset..offset + 64) {
            Some(c) => c,
            None => break,
        };
        process_block(&mut state, &mut w, chunk);
        offset += 64;
    }

    // Prepare final padded block(s)
    let remainder = total_len - offset;
    let mut tail = [0u8; 128];
    if let Some(src) = data.get(offset..) {
        if let Some(dst) = tail.get_mut(..remainder) {
            dst.copy_from_slice(src);
        }
    }
    if let Some(pad_byte) = tail.get_mut(remainder) {
        *pad_byte = 0x80;
    }

    if remainder < 56 {
        // Fits in a single 64-byte block
        let bit_bytes = bit_len.to_be_bytes();
        if let Some(len_dst) = tail.get_mut(56..64) {
            len_dst.copy_from_slice(&bit_bytes);
        }
        if let Some(block) = tail.get(..64) {
            process_block(&mut state, &mut w, block);
        }
    } else {
        // Requires two 64-byte blocks
        let bit_bytes = bit_len.to_be_bytes();
        if let Some(len_dst) = tail.get_mut(120..128) {
            len_dst.copy_from_slice(&bit_bytes);
        }
        if let Some(block0) = tail.get(..64) {
            process_block(&mut state, &mut w, block0);
        }
        if let Some(block1) = tail.get(64..128) {
            process_block(&mut state, &mut w, block1);
        }
    }

    // Convert state to 32-byte big-endian output
    let mut output = [0u8; 32];
    for (i, word) in state.iter().enumerate() {
        let bytes = word.to_be_bytes();
        if let Some(dst) = output.get_mut(i * 4..(i + 1) * 4) {
            dst.copy_from_slice(&bytes);
        }
    }
    output
}

#[inline(always)]
fn process_block(state: &mut [u32; 8], w: &mut [u32; 64], block: &[u8]) {
    for t in 0..16 {
        let idx = t * 4;
        let b0 = block.get(idx).copied().unwrap_or(0) as u32;
        let b1 = block.get(idx + 1).copied().unwrap_or(0) as u32;
        let b2 = block.get(idx + 2).copied().unwrap_or(0) as u32;
        let b3 = block.get(idx + 3).copied().unwrap_or(0) as u32;
        if let Some(slot) = w.get_mut(t) {
            *slot = (b0 << 24) | (b1 << 16) | (b2 << 8) | b3;
        }
    }

    for t in 16..64 {
        let w_t2 = w.get(t - 2).copied().unwrap_or(0);
        let w_t7 = w.get(t - 7).copied().unwrap_or(0);
        let w_t15 = w.get(t - 15).copied().unwrap_or(0);
        let w_t16 = w.get(t - 16).copied().unwrap_or(0);
        if let Some(slot) = w.get_mut(t) {
            *slot = gamma1(w_t2)
                .wrapping_add(w_t7)
                .wrapping_add(gamma0(w_t15))
                .wrapping_add(w_t16);
        }
    }

    let mut a = state[0];
    let mut b = state[1];
    let mut c = state[2];
    let mut d = state[3];
    let mut e = state[4];
    let mut f = state[5];
    let mut g = state[6];
    let mut h = state[7];

    for t in 0..64 {
        let wt = w.get(t).copied().unwrap_or(0);
        let kt = K.get(t).copied().unwrap_or(0);
        let t1 = h
            .wrapping_add(sigma1(e))
            .wrapping_add(ch(e, f, g))
            .wrapping_add(kt)
            .wrapping_add(wt);
        let t2 = sigma0(a).wrapping_add(maj(a, b, c));
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}

/// Computes the HMAC-SHA256 of `data` using `key` according to RFC 2104.
///
/// Uses zero heap allocations and operates strictly in fixed stack buffers.
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut k_pad = [0u8; 64];

    if key.len() > 64 {
        let hashed_key = sha256(key);
        if let Some(dst) = k_pad.get_mut(..32) {
            dst.copy_from_slice(&hashed_key);
        }
    } else if let Some(dst) = k_pad.get_mut(..key.len()) {
        dst.copy_from_slice(key);
    }

    // Inner hash: H((K ^ ipad) || data)
    let mut inner_buffer = [0u8; 64 + 1024]; // Supports up to 1024 bytes data on stack
    let inner_hash = if data.len() <= 1024 {
        for (i, byte) in k_pad.iter().enumerate() {
            if let Some(slot) = inner_buffer.get_mut(i) {
                *slot = byte ^ 0x36;
            }
        }
        if let Some(dst) = inner_buffer.get_mut(64..64 + data.len()) {
            dst.copy_from_slice(data);
        }
        sha256(inner_buffer.get(..64 + data.len()).unwrap_or(&[]))
    } else {
        // Fallback streaming for large data buffers
        let mut inner_prefix = [0u8; 64];
        for (i, byte) in k_pad.iter().enumerate() {
            if let Some(slot) = inner_prefix.get_mut(i) {
                *slot = byte ^ 0x36;
            }
        }
        sha256_concat(&inner_prefix, data)
    };

    // Outer hash: H((K ^ opad) || inner_hash)
    let mut outer_buffer = [0u8; 64 + 32];
    for (i, byte) in k_pad.iter().enumerate() {
        if let Some(slot) = outer_buffer.get_mut(i) {
            *slot = byte ^ 0x5C;
        }
    }
    if let Some(dst) = outer_buffer.get_mut(64..64 + 32) {
        dst.copy_from_slice(&inner_hash);
    }

    sha256(&outer_buffer)
}

/// Helper function to compute SHA-256 over two contiguous byte slices without heap allocation.
fn sha256_concat(a: &[u8], b: &[u8]) -> [u8; 32] {
    let total_len = a.len() + b.len();
    let bit_len = (total_len as u64).wrapping_mul(8);
    let mut state = H_INIT;
    let mut w = [0u32; 64];

    let mut buf = [0u8; 64];
    let mut buf_len = 0;

    let process_byte = |byte: u8,
                        buf: &mut [u8; 64],
                        buf_len: &mut usize,
                        state: &mut [u32; 8],
                        w: &mut [u32; 64]| {
        if let Some(slot) = buf.get_mut(*buf_len) {
            *slot = byte;
            *buf_len += 1;
            if *buf_len == 64 {
                process_block(state, w, buf);
                *buf_len = 0;
            }
        }
    };

    for &byte in a {
        process_byte(byte, &mut buf, &mut buf_len, &mut state, &mut w);
    }
    for &byte in b {
        process_byte(byte, &mut buf, &mut buf_len, &mut state, &mut w);
    }

    // Append 0x80
    process_byte(0x80, &mut buf, &mut buf_len, &mut state, &mut w);

    if buf_len > 56 {
        while buf_len < 64 {
            if let Some(slot) = buf.get_mut(buf_len) {
                *slot = 0;
                buf_len += 1;
            }
        }
        process_block(&mut state, &mut w, &buf);
        buf_len = 0;
    }

    while buf_len < 56 {
        if let Some(slot) = buf.get_mut(buf_len) {
            *slot = 0;
            buf_len += 1;
        }
    }

    let bit_bytes = bit_len.to_be_bytes();
    if let Some(dst) = buf.get_mut(56..64) {
        dst.copy_from_slice(&bit_bytes);
    }
    process_block(&mut state, &mut w, &buf);

    let mut output = [0u8; 32];
    for (i, word) in state.iter().enumerate() {
        let bytes = word.to_be_bytes();
        if let Some(dst) = output.get_mut(i * 4..(i + 1) * 4) {
            dst.copy_from_slice(&bytes);
        }
    }
    output
}

/// Constant-time 32-byte equality check to prevent timing side-channel attacks.
#[inline]
pub fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 {
        let byte_a = a.get(i).copied().unwrap_or(0);
        let byte_b = b.get(i).copied().unwrap_or(0);
        diff |= byte_a ^ byte_b;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nist_sha256_empty_string() {
        let hash = sha256(b"");
        let hex = to_hex(&hash);
        assert_eq!(
            hex,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_nist_sha256_abc() {
        let hash = sha256(b"abc");
        let hex = to_hex(&hash);
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn test_nist_sha256_long_string() {
        let hash = sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq");
        let hex = to_hex(&hash);
        assert_eq!(
            hex,
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn test_rfc4231_hmac_sha256_case1() {
        let key = [0x0bu8; 20];
        let data = b"Hi There";
        let mac = hmac_sha256(&key, data);
        let hex = to_hex(&mac);
        assert_eq!(
            hex,
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn test_rfc4231_hmac_sha256_case2() {
        let key = b"Jefe";
        let data = b"what do ya want for nothing?";
        let mac = hmac_sha256(key, data);
        let hex = to_hex(&mac);
        assert_eq!(
            hex,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn test_constant_time_eq() {
        let a = [42u8; 32];
        let mut b = [42u8; 32];
        assert!(constant_time_eq(&a, &b));
        b[31] = 43;
        assert!(!constant_time_eq(&a, &b));
    }

    fn to_hex(bytes: &[u8; 32]) -> String {
        let mut s = String::with_capacity(64);
        for &b in bytes {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
        }
        s
    }
}
