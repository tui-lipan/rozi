//! SHA-256, for verifying downloaded release archives before installing them on a remote host.
//!
//! Implemented here rather than shelled out to `sha256sum`/`shasum`, which do not exist on Windows,
//! or added as a dependency, which would pull a transitive tree into an otherwise small manifest
//! for one fixed algorithm. Verification is the security-relevant step of a cross-platform install,
//! so it must behave identically everywhere and never silently skip for want of a tool.
//!
//! FIPS 180-4. Covered by the standard test vectors below.

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

const INITIAL: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// Streaming SHA-256, so a release archive is hashed without being read into memory whole.
pub struct Sha256 {
    state: [u32; 8],
    /// Partial block carried between updates; always fewer than 64 bytes.
    buffer: [u8; 64],
    buffered: usize,
    /// Total message length in bytes, for the length suffix.
    length: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Self {
        Self {
            state: INITIAL,
            buffer: [0u8; 64],
            buffered: 0,
            length: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);

        // Top up a partial block first, so the fast path below always starts block-aligned.
        if self.buffered > 0 {
            let take = (64 - self.buffered).min(data.len());
            self.buffer[self.buffered..self.buffered + take].copy_from_slice(&data[..take]);
            self.buffered += take;
            data = &data[take..];
            if self.buffered < 64 {
                // Still short of a full block, which means `data` is now empty. Returning here is
                // load-bearing: the tail below assigns `buffered` from the leftover slice and would
                // otherwise reset this partial block's count to zero and lose it.
                return;
            }
            let block = self.buffer;
            self.compress(&block);
            self.buffered = 0;
        }

        let mut chunks = data.chunks_exact(64);
        for block in &mut chunks {
            let mut fixed = [0u8; 64];
            fixed.copy_from_slice(block);
            self.compress(&fixed);
        }

        let rest = chunks.remainder();
        self.buffer[..rest.len()].copy_from_slice(rest);
        self.buffered = rest.len();
    }

    /// Finish and return the digest as lowercase hex, the form checksum files carry.
    pub fn finalize_hex(mut self) -> String {
        let bit_length = self.length.wrapping_mul(8);

        // Padding: a single 1 bit, zeroes, then the 64-bit big-endian bit length.
        self.buffer[self.buffered] = 0x80;
        self.buffered += 1;
        if self.buffered > 56 {
            self.buffer[self.buffered..].fill(0);
            let block = self.buffer;
            self.compress(&block);
            self.buffered = 0;
        }
        self.buffer[self.buffered..56].fill(0);
        self.buffer[56..].copy_from_slice(&bit_length.to_be_bytes());
        let block = self.buffer;
        self.compress(&block);

        let mut hex = String::with_capacity(64);
        for word in self.state {
            use std::fmt::Write;
            let _ = write!(hex, "{word:08x}");
        }
        hex
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (index, slot) in w.iter_mut().take(16).enumerate() {
            let start = index * 4;
            *slot = u32::from_be_bytes([
                block[start],
                block[start + 1],
                block[start + 2],
                block[start + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(choice)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(majority);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        for (slot, value) in self
            .state
            .iter_mut()
            .zip([a, b, c, d, e, f, g, h].into_iter())
        {
            *slot = slot.wrapping_add(value);
        }
    }
}

/// Hash a file without reading it into memory whole.
pub fn hash_file(path: &std::path::Path) -> std::io::Result<String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hasher.finalize_hex())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        hasher.finalize_hex()
    }

    /// FIPS 180-4 / NIST published vectors. These are the whole basis for trusting this module.
    #[test]
    fn matches_published_vectors() {
        assert_eq!(
            hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            hex(b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu"),
            "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1"
        );
    }

    #[test]
    fn matches_the_million_a_vector() {
        let mut hasher = Sha256::new();
        for _ in 0..1_000 {
            hasher.update(&[b'a'; 1_000]);
        }
        assert_eq!(
            hasher.finalize_hex(),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    /// Chunked feeding must not change the digest — the streaming path is what `hash_file` uses,
    /// and a buffering bug here would silently accept a corrupt download.
    #[test]
    fn streaming_matches_one_shot_at_every_split() {
        let data: Vec<u8> = (0..300u32).map(|byte| (byte % 251) as u8).collect();
        let expected = hex(&data);
        for split in 0..data.len() {
            let mut hasher = Sha256::new();
            hasher.update(&data[..split]);
            hasher.update(&data[split..]);
            assert_eq!(hasher.finalize_hex(), expected, "split at {split}");
        }
    }

    /// 55/56/57 and 63/64/65 straddle the padding and block boundaries.
    #[test]
    fn handles_padding_boundary_lengths() {
        for length in [54usize, 55, 56, 57, 63, 64, 65, 119, 120, 128] {
            let data = vec![b'x'; length];
            let mut chunked = Sha256::new();
            for byte in &data {
                chunked.update(std::slice::from_ref(byte));
            }
            assert_eq!(
                chunked.finalize_hex(),
                hex(&data),
                "byte-at-a-time differed at length {length}"
            );
        }
    }

    #[test]
    fn hashes_a_file_from_disk() {
        let path = std::env::temp_dir().join(format!("hyprmux-sha256-{}", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(
            hash_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_file(&path);
    }
}
