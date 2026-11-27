//! Ogg page writer (and a reader for checking it).

fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    for (i, slot) in t.iter_mut().enumerate() {
        let mut r = (i as u32) << 24;
        for _ in 0..8 {
            r = if r & 0x8000_0000 != 0 {
                (r << 1) ^ 0x04C1_1DB7
            } else {
                r << 1
            };
        }
        *slot = r;
    }
    t
}

/// Ogg's CRC-32: polynomial 0x04C11DB7, no reflection, zero start and no final xor.
pub fn crc32(data: &[u8]) -> u32 {
    let t = crc_table();
    data.iter()
        .fold(0u32, |crc, &b| (crc << 8) ^ t[((crc >> 24) as u8 ^ b) as usize])
}

pub const FLAG_CONTINUED: u8 = 1;
pub const FLAG_BOS: u8 = 2;
pub const FLAG_EOS: u8 = 4;

/// Writes one logical stream.
pub struct OggWriter {
    serial: u32,
    sequence: u32,
    out: Vec<u8>,
}

impl OggWriter {
    pub fn new(serial: u32) -> Self {
        Self {
            serial,
            sequence: 0,
            out: Vec::new(),
        }
    }

    /// Write `packets` as one page whose last packet ends at `granule`. Packets must fit the 255-segment limit
    /// of a page together; the caller splits otherwise.
    pub fn page(&mut self, flags: u8, granule: i64, packets: &[&[u8]]) {
        let mut segments = Vec::new();
        for p in packets {
            let mut left = p.len();
            while left >= 255 {
                segments.push(255u8);
                left -= 255;
            }
            segments.push(left as u8);
        }
        assert!(segments.len() <= 255, "too many segments for one Ogg page");
        let start = self.out.len();
        self.out.extend_from_slice(b"OggS");
        self.out.push(0);
        self.out.push(flags);
        self.out.extend_from_slice(&granule.to_le_bytes());
        self.out.extend_from_slice(&self.serial.to_le_bytes());
        self.out.extend_from_slice(&self.sequence.to_le_bytes());
        self.out.extend_from_slice(&[0; 4]);
        self.out.push(segments.len() as u8);
        self.out.extend_from_slice(&segments);
        for p in packets {
            self.out.extend_from_slice(p);
        }
        let crc = crc32(&self.out[start..]);
        self.out[start + 22..start + 26].copy_from_slice(&crc.to_le_bytes());
        self.sequence += 1;
    }

    /// How many packets of these lengths fit in one page.
    pub fn packets_per_page(lengths: &[usize]) -> usize {
        let mut segs = 0;
        for (i, &l) in lengths.iter().enumerate() {
            segs += l / 255 + 1;
            if segs > 255 {
                return i.max(1);
            }
        }
        lengths.len()
    }

    pub fn finish(self) -> Vec<u8> {
        self.out
    }
}

/// A page as read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub flags: u8,
    pub granule: i64,
    pub serial: u32,
    pub sequence: u32,
    pub packets: Vec<Vec<u8>>,
}

/// Reads all pages of a single-stream file whose packets do not span pages. Verifies each page checksum.
pub fn read_pages(bytes: &[u8]) -> crate::Result<Vec<Page>> {
    let mut pages = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let h = bytes
            .get(at..at + 27)
            .filter(|h| &h[..4] == b"OggS")
            .ok_or_else(|| crate::Error::Invalid("bad Ogg page header".into()))?;
        let nsegs = h[26] as usize;
        let table = bytes
            .get(at + 27..at + 27 + nsegs)
            .ok_or_else(|| crate::Error::Invalid("truncated Ogg segment table".into()))?;
        let body_len: usize = table.iter().map(|&s| s as usize).sum();
        let end = at + 27 + nsegs + body_len;
        let page = bytes
            .get(at..end)
            .ok_or_else(|| crate::Error::Invalid("truncated Ogg page".into()))?;
        let mut check = page.to_vec();
        check[22..26].fill(0);
        let stored = u32::from_le_bytes([h[22], h[23], h[24], h[25]]);
        if crc32(&check) != stored {
            return Err(crate::Error::Invalid("Ogg page checksum mismatch".into()));
        }
        let mut packets = Vec::new();
        let mut cur = Vec::new();
        let mut p = at + 27 + nsegs;
        for &s in table {
            cur.extend_from_slice(&bytes[p..p + s as usize]);
            p += s as usize;
            if s < 255 {
                packets.push(std::mem::take(&mut cur));
            }
        }
        pages.push(Page {
            flags: h[5],
            granule: i64::from_le_bytes(h[6..14].try_into().expect("8 bytes")),
            serial: u32::from_le_bytes([h[14], h[15], h[16], h[17]]),
            sequence: u32::from_le_bytes([h[18], h[19], h[20], h[21]]),
            packets,
        });
        at = end;
    }
    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_of_known_vector() {
        // CRC-32/MPEG-2 style variant without final xor: check value of "123456789" is 0x89A1897F
        assert_eq!(crc32(b"123456789"), 0x89A1_897F);
    }

    #[test]
    fn pages_round_trip_including_long_packets() {
        let mut w = OggWriter::new(0xABCD);
        let long = vec![7u8; 600];
        let empty: &[u8] = &[];
        w.page(FLAG_BOS, 0, &[b"head"]);
        w.page(0, 960, &[&long, b"x", empty]);
        w.page(FLAG_EOS, 1920, &[&[1, 2, 3]]);
        let bytes = w.finish();
        let pages = read_pages(&bytes).unwrap();
        assert_eq!(pages.len(), 3);
        assert_eq!(pages[0].flags, FLAG_BOS);
        assert_eq!(pages[1].packets, vec![long, b"x".to_vec(), vec![]]);
        assert_eq!(pages[2].granule, 1920);
        assert_eq!(pages[2].sequence, 2);
        assert_eq!(pages[2].serial, 0xABCD);
        let mut bad = bytes.clone();
        let n = bad.len();
        bad[n - 1] ^= 1;
        assert!(read_pages(&bad).is_err());
    }

    #[test]
    fn page_capacity_counts_lacing() {
        assert_eq!(OggWriter::packets_per_page(&[10; 300]), 255);
        assert_eq!(OggWriter::packets_per_page(&[600, 600]), 2);
        assert_eq!(OggWriter::packets_per_page(&[255 * 200, 255 * 100]), 1);
        assert_eq!(OggWriter::packets_per_page(&[]), 0);
    }
}
