// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! RFC 1321 MD5, streamed. Bambu Lab packages carry the plate G-code's MD5 as
//! a byte-consistency check the firmware reads; it is not a security digest.

pub struct Md5 {
    state: [u32; 4],
    buffer: [u8; 64],
    buffered: usize,
    length: u64,
}

const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

impl Default for Md5 {
    fn default() -> Self {
        Self {
            state: [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476],
            buffer: [0; 64],
            buffered: 0,
            length: 0,
        }
    }
}

impl Md5 {
    pub fn update(&mut self, mut data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        while !data.is_empty() {
            let take = (64 - self.buffered).min(data.len());
            self.buffer[self.buffered..self.buffered + take].copy_from_slice(&data[..take]);
            self.buffered += take;
            data = &data[take..];
            if self.buffered == 64 {
                let block = self.buffer;
                self.block(&block);
                self.buffered = 0;
            }
        }
    }

    fn block(&mut self, block: &[u8; 64]) {
        let m: [u32; 16] = std::array::from_fn(|i| {
            u32::from_le_bytes(block[i * 4..i * 4 + 4].try_into().unwrap())
        });
        let [mut a, mut b, mut c, mut d] = self.state;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let rotated = a
                .wrapping_add(f)
                .wrapping_add(K[i])
                .wrapping_add(m[g])
                .rotate_left(S[i]);
            (a, d, c) = (d, c, b);
            b = b.wrapping_add(rotated);
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d]) {
            *s = s.wrapping_add(v);
        }
    }

    pub fn finish(mut self) -> [u8; 16] {
        let bits = self.length.wrapping_mul(8);
        self.update(&[0x80]);
        while self.buffered != 56 {
            self.update(&[0]);
        }
        self.update(&bits.to_le_bytes());
        let mut out = [0u8; 16];
        for (chunk, word) in out.chunks_mut(4).zip(self.state) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        out
    }
}

/// Uppercase hexadecimal, as Bambu's `plate_N.gcode.md5` member stores it.
pub fn upper_hex(digest: &[u8; 16]) -> String {
    digest.iter().map(|b| format!("{b:02X}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn md5(data: &[u8]) -> String {
        let mut h = Md5::default();
        h.update(data);
        upper_hex(&h.finish())
    }
    #[test]
    fn rfc1321_vectors() {
        assert_eq!(md5(b""), "D41D8CD98F00B204E9800998ECF8427E");
        assert_eq!(md5(b"abc"), "900150983CD24FB0D6963F7D28E17F72");
        assert_eq!(
            md5(
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"
            ),
            "57EDF4A22BE3C955AC49DA2E2107B67A"
        );
        // Streaming across block boundaries matches one update.
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7) as u8).collect();
        let mut h = Md5::default();
        for part in data.chunks(13) {
            h.update(part);
        }
        assert_eq!(upper_hex(&h.finish()), md5(&data));
    }
}
