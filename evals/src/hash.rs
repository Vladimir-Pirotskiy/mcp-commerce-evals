//! SHA-256 (FIPS 180-4), used to fingerprint the queries file and the system prompt so a report
//! says exactly what input produced it. A hundred lines here beat a dependency for one digest.

use std::fmt::Write;

/// Bytes in one message block.
const BLOCK: usize = 64;
/// Bytes at the end of the last block that hold the message length in bits.
const LENGTH_BYTES: usize = 8;

const ROUND_CONSTANTS: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const INITIAL_STATE: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// Mixes one 64-byte block into `state`.
fn compress(state: &mut [u32; 8], block: &[u8; BLOCK]) {
    let mut schedule = [0u32; 64];
    for (word, bytes) in schedule.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_be_bytes(*bytes);
    }
    for i in 16..64 {
        let s0 = schedule[i - 15].rotate_right(7)
            ^ schedule[i - 15].rotate_right(18)
            ^ (schedule[i - 15] >> 3);
        let s1 = schedule[i - 2].rotate_right(17)
            ^ schedule[i - 2].rotate_right(19)
            ^ (schedule[i - 2] >> 10);
        schedule[i] = schedule[i - 16]
            .wrapping_add(s0)
            .wrapping_add(schedule[i - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for (constant, word) in ROUND_CONSTANTS.iter().zip(schedule) {
        let sigma1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let choice = (e & f) ^ (!e & g);
        let t1 = h
            .wrapping_add(sigma1)
            .wrapping_add(choice)
            .wrapping_add(*constant)
            .wrapping_add(word);
        let sigma0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let majority = (a & b) ^ (a & c) ^ (b & c);
        let t2 = sigma0.wrapping_add(majority);
        (h, g, f, e) = (g, f, e, d.wrapping_add(t1));
        (d, c, b, a) = (c, b, a, t1.wrapping_add(t2));
    }
    for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *slot = slot.wrapping_add(value);
    }
}

/// The SHA-256 digest of `data` as 64 lowercase hex digits.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % BLOCK != BLOCK - LENGTH_BYTES {
        padded.push(0);
    }
    let bits = (data.len() as u64).wrapping_mul(8);
    padded.extend_from_slice(&bits.to_be_bytes());
    let mut state = INITIAL_STATE;
    for block in padded.as_chunks::<BLOCK>().0 {
        compress(&mut state, block);
    }
    state
        .iter()
        .fold(String::with_capacity(64), |mut hex, word| {
            let _ = write!(hex, "{word:08x}");
            hex
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_published_test_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn matches_shasum_around_the_padding_boundaries() {
        let cases = [
            (
                55,
                "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318",
            ),
            (
                56,
                "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a",
            ),
            (
                64,
                "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb",
            ),
            (
                1000,
                "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3",
            ),
        ];
        for (len, digest) in cases {
            assert_eq!(sha256_hex(&vec![b'a'; len]), digest, "{len} bytes");
        }
    }
}
