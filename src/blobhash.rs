/*!
The git blob object id of a byte string, computed in process.

A snapshot object in the `.rr/` sidecar is named by its content hash, and that
name is git's blob id: `SHA-1("blob " + len + "\0" + bytes)` (git uses SHA-1 for
object ids in a default repository). Computing it here, with no git and no crypto
dependency, is what lets `rr read <anchor>@<commit>` verify a recovered object
against its name without shelling out to git or depending on the commit still
being reachable. `rr cite` takes the same id from `git hash-object` when it
writes the object, so the two agree by construction (and every snapshot
round-trip test proves it).

SHA-1 is a content address here, not a security boundary: a blob id only has to
match git's, byte for byte, which the tests pin against known git object ids.
The implementation is a direct transcription of FIPS 180-1.
*/

/// The git blob object id of `bytes`, as 40 lowercase hex characters.
pub fn blob_oid(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(format!("blob {}\0", bytes.len()).as_bytes());
    hasher.update(bytes);
    to_hex(&hasher.finalize())
}

/// Streaming SHA-1 state: the five 32-bit words, a 64-byte block buffer, and the
/// running message length (bytes hashed so far) for the final length padding.
struct Sha1 {
    state: [u32; 5],
    block: [u8; 64],
    block_len: usize,
    msg_len: u64,
}

impl Sha1 {
    fn new() -> Sha1 {
        Sha1 {
            state: [
                0x6745_2301,
                0xEFCD_AB89,
                0x98BA_DCFE,
                0x1032_5476,
                0xC3D2_E1F0,
            ],
            block: [0; 64],
            block_len: 0,
            msg_len: 0,
        }
    }

    /// Absorb more input, processing each full 64-byte block as it fills.
    fn update(&mut self, mut data: &[u8]) {
        self.msg_len = self.msg_len.wrapping_add(data.len() as u64);
        while !data.is_empty() {
            let take = (64 - self.block_len).min(data.len());
            self.block[self.block_len..self.block_len + take].copy_from_slice(&data[..take]);
            self.block_len += take;
            data = &data[take..];
            if self.block_len == 64 {
                self.process_block();
                self.block_len = 0;
            }
        }
    }

    /// Append the `0x80` terminator, the zero padding, and the 64-bit big-endian
    /// bit length, then return the final 20-byte digest.
    fn finalize(mut self) -> [u8; 20] {
        let bit_len = self.msg_len.wrapping_mul(8);
        self.update(&[0x80]);
        while self.block_len != 56 {
            self.update(&[0x00]);
        }
        self.update(&bit_len.to_be_bytes());
        debug_assert_eq!(
            self.block_len, 0,
            "length padding lands on a block boundary"
        );

        let mut out = [0u8; 20];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    /// The 80-round SHA-1 compression over the current 64-byte block.
    fn process_block(&mut self) {
        let mut w = [0u32; 80];
        for (i, chunk) in self.block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let [mut a, mut b, mut c, mut d, mut e] = self.state;
        for (i, &word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
    }
}

/// Lowercase hex encoding of a digest.
fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every expected value is what `git hash-object --stdin` produces for that
    // exact content, so these pin our blob id to git's byte for byte.
    #[test]
    fn matches_git_blob_ids() {
        assert_eq!(blob_oid(b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
        assert_eq!(
            blob_oid(b"hello\n"),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
        assert_eq!(blob_oid(b"hi"), "32f95c0d1244a78b2be1bab8de17906fabb2c4a8");
        assert_eq!(
            blob_oid(b"The quick brown fox jumps over the lazy dog"),
            "ff3bb63948b4b24796d2acd259915f2a9d972638"
        );
        // > 64 bytes, so SHA-1 processes more than one block.
        assert_eq!(
            blob_oid(b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghij"),
            "4bec353844d95b792368b304242ab1915c8cf195"
        );
        // Embedded NULs: the hash is over raw bytes, not a C string.
        assert_eq!(
            blob_oid(b"a\0b\0c\n"),
            "5892d4f6cca6f5e07e2ddfba54bb4c814b7601a7"
        );
    }

    // Lengths around the 64-byte block boundary and the 56-byte padding
    // threshold, where an off-by-one in the padding would first show up. Each
    // expected value is `git hash-object` of that exact byte string.
    #[test]
    fn handles_block_boundary_lengths() {
        assert_eq!(
            blob_oid(&[b'z'; 55]),
            "4a41bc657cf0c93685ee7183b2538068078e8a5f"
        );
        assert_eq!(
            blob_oid(&[b'y'; 64]),
            "e53516416fbf4f7904d4674c1d099ffd386ca249"
        );
        assert_eq!(
            blob_oid(&[b'x'; 100]),
            "f6be7cae2045aac11912ea642bf7f9d5d261f63b"
        );
    }
}
