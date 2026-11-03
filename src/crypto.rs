//! 手書きの sha256 / base64 / 乱数（crate を足さない規約: CD-31）。std と serde 以外に依存しない。
//! bin（src/bin/kangae-google.rs）と tests/google.rs は `#[path]` でこの file を crate の根に取り込む（src/google が `crate::crypto` で引く）。
#![allow(dead_code)] // main の側で使うのは src/google を配線してから

/// SHA-256（FIPS 180-4）。RFC 7636 Appendix B の octets と NIST の例で tests/google.rs が測る
pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01,
        0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
        0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147,
        0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08,
        0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
        0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 { msg.push(0) }
    msg.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_be_bytes());
    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks_exact(4).enumerate() { w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]) }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let t1 = hh.wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25)).wrapping_add((e & f) ^ (!e & g)).wrapping_add(K[i]).wrapping_add(w[i]);
            let t2 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22)).wrapping_add((a & b) ^ (a & c) ^ (b & c));
            (hh, g, f, e, d, c, b, a) = (g, f, e, d.wrapping_add(t1), c, b, a, t1.wrapping_add(t2));
        }
        for (x, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) { *x = x.wrapping_add(v) }
    }
    let mut out = [0u8; 32];
    for (chunk, v) in out.chunks_exact_mut(4).zip(h) { chunk.copy_from_slice(&v.to_be_bytes()) }
    out
}
pub fn sha256_hex(data: &[u8]) -> String { sha256(data).iter().map(|b| format!("{b:02x}")).collect() }

/// HMAC-SHA256（RFC 2104・鍵が 64 byte を越えれば sha256 してから使う）。ENGINE-AUDIT W7 本体 3: 同意の file の行の署名（`surface::sealed`）。
/// 例: `hmac_sha256(&[0x0b; 20], b"Hi There")` → RFC 4231 の Test Case 1 の値
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let _ = (key, data);
    todo!("ENGINE-AUDIT W7 本体 3")
}

#[cfg(test)]
mod w7_tests {
    /// W7 本体 3: RFC 4231 の Test Case 1・2・6（鍵が 64 byte を越える）。負の対照: 長い鍵を sha256 しない → 3 つ目が赤・opad を ipad にする → 全部赤。
    #[test]
    fn w7_hmac_sha256_matches_rfc_4231() {
        let hex = |b: [u8; 32]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(hex(super::hmac_sha256(&[0x0b; 20], b"Hi There")), "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
        assert_eq!(hex(super::hmac_sha256(b"Jefe", b"what do ya want for nothing?")), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        assert_eq!(
            hex(super::hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }
}

/// RFC 4648。url_safe は Gmail の raw と PKCE（base64url）、標準は RFC 2047 の件名。padding は付ける（要らない側が trim する）
pub fn base64(data: &[u8], url_safe: bool) -> String {
    let table: &[u8; 64] = if url_safe { b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_" }
                           else { b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/" };
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(table[(n >> 18) as usize & 63] as char);
        out.push(table[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { table[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { table[n as usize & 63] as char } else { '=' });
    }
    out
}
/// base64 と base64url の両方を読む（padding は有っても無くてもよい・改行は飛ばす）。それ以外の文字は None
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'), b'a'..=b'z' => Some(c - b'a' + 26), b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' | b'-' => Some(62), b'/' | b'_' => Some(63), _ => None,
    };
    let digits: Vec<u8> = s.bytes().filter(|c| !matches!(c, b'\r' | b'\n' | b'=')).map(val).collect::<Option<_>>()?;
    if digits.len() % 4 == 1 { return None }
    let mut out = Vec::with_capacity(digits.len() * 3 / 4);
    for chunk in digits.chunks(4) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &d)| n | (d as u32) << (18 - 6 * i));
        out.extend_from_slice(&n.to_be_bytes()[1..chunk.len()]);
    }
    Some(out)
}

/// /dev/urandom から n byte
pub fn random(n: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut buf = vec![0u8; n];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf)).map_err(|e| format!("乱数を取れません（{e}）。"))?;
    Ok(buf)
}
