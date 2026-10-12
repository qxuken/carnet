//! A ZIP file of a few files, stored as they are (no compression): what a
//! browser removed from its account hands over of the notes it hadn't
//! sent (web/, `sync.svelte.ts`). Every system opens one; Markdown and
//! pictures don't shrink enough to be worth more.

/// The files, by path (`/` between folders), as one ZIP file's bytes.
pub fn zip(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let at = out.len() as u32;
        let (crc, len, name) = (crc32(data), data.len() as u32, name.as_bytes());
        // What the local header and the central directory's entry share:
        // made by 2.0, UTF-8 names, stored, 1980-01-01 00:00.
        let mut common = Vec::new();
        for v in [20u16, 0x0800, 0, 0, 0x21] {
            common.extend(v.to_le_bytes());
        }
        for v in [crc, len, len] {
            common.extend(v.to_le_bytes());
        }
        common.extend((name.len() as u16).to_le_bytes());
        common.extend(0u16.to_le_bytes());
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend(&common);
        out.extend(name);
        out.extend(data);
        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(&common);
        // No comment, disk 0, no attributes; then where its header is.
        for v in [0u16, 0, 0] {
            central.extend(v.to_le_bytes());
        }
        central.extend(0u32.to_le_bytes());
        central.extend(at.to_le_bytes());
        central.extend(name);
    }
    let (at, size, n) = (out.len() as u32, central.len() as u32, files.len() as u16);
    out.extend(central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    for v in [0u16, 0, n, n] {
        out.extend(v.to_le_bytes());
    }
    out.extend(size.to_le_bytes());
    out.extend(at.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

/// ZIP's checksum (CRC-32, as zlib's).
fn crc32(bytes: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in bytes {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 == 1 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checksum_is_zlibs() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    /// Read back as an unzipper reads one: from the directory at its end,
    /// each entry to its header and bytes.
    #[test]
    fn files_come_back_as_they_went_in() {
        let files = vec![("Trips/Packing list.md".to_string(), b"# Packing list\n\npassport\n".to_vec()), ("attachments/p\u{e9}.png".to_string(), vec![0, 1, 2, 255])];
        let z = zip(&files);
        let u16_at = |i: usize| u16::from_le_bytes([z[i], z[i + 1]]) as usize;
        let u32_at = |i: usize| u32::from_le_bytes(z[i..i + 4].try_into().unwrap()) as usize;
        let end = z.len() - 22;
        assert_eq!(u32_at(end), 0x0605_4b50);
        assert_eq!(u16_at(end + 10), 2);
        let mut at = u32_at(end + 16);
        for (name, data) in &files {
            assert_eq!(u32_at(at), 0x0201_4b50);
            let (len, name_len, local) = (u32_at(at + 24), u16_at(at + 28), u32_at(at + 42));
            assert_eq!(&z[at + 46..at + 46 + name_len], name.as_bytes());
            assert_eq!(u32_at(at + 16), crc32(data) as usize);
            assert_eq!(u32_at(local), 0x0403_4b50);
            let body = local + 30 + u16_at(local + 26);
            assert_eq!(&z[body..body + len], &data[..]);
            at += 46 + name_len;
        }
    }
}
