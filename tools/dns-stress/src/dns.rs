//! DNS wire validation with bounded decompression and no unsafe code.

use anyhow::{Result, bail, ensure};

#[derive(Debug)]
pub(crate) struct Name {
    pub(crate) wire: Vec<u8>,
    pub(crate) query: Vec<u8>,
}

impl Name {
    pub(crate) fn parse(text: &str, recursive: bool) -> Result<Self> {
        let text = text.strip_suffix('.').unwrap_or(text);
        ensure!(!text.is_empty(), "root/empty query name is unsupported");
        let mut wire = Vec::with_capacity(text.len() + 2);
        for label in text.split('.') {
            ensure!(
                !label.is_empty() && label.len() <= 63,
                "invalid label length"
            );
            ensure!(
                label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
                "names must contain ASCII letters, digits, hyphens, or underscores"
            );
            wire.push(u8::try_from(label.len())?);
            wire.extend(label.bytes().map(|b| b.to_ascii_lowercase()));
        }
        wire.push(0);
        ensure!(wire.len() <= 255, "DNS name exceeds 255 wire bytes");
        let mut query = vec![0; 12];
        query[2] = u8::from(recursive);
        query[5] = 1;
        query.extend_from_slice(&wire);
        query.extend_from_slice(&[0, 1, 0, 1]);
        Ok(Self { wire, query })
    }
}

#[derive(Debug)]
pub(crate) struct Response {
    pub(crate) id: u16,
    pub(crate) flags: u16,
    pub(crate) answers: u16,
    pub(crate) question: [u8; 255],
    pub(crate) question_len: usize,
}

fn word(packet: &[u8], offset: usize) -> Result<u16> {
    let bytes = packet
        .get(offset..offset + 2)
        .ok_or_else(|| anyhow::anyhow!("short DNS field"))?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn decode_name(packet: &[u8], cursor: &mut usize, out: &mut [u8; 255]) -> Result<usize> {
    let mut offset = *cursor;
    let mut jumped = false;
    let mut size = 0;
    for _ in 0..128 {
        let length = *packet
            .get(offset)
            .ok_or_else(|| anyhow::anyhow!("short DNS name"))?;
        if length & 0xc0 == 0xc0 {
            let pointer = usize::from(word(packet, offset)? & 0x3fff);
            ensure!(pointer < offset, "forward/cyclic DNS pointer");
            if !jumped {
                *cursor = offset + 2;
                jumped = true;
            }
            offset = pointer;
        } else {
            ensure!(length <= 63, "reserved DNS label encoding");
            let length = usize::from(length);
            ensure!(size + length < out.len(), "expanded DNS name too long");
            let label = packet
                .get(offset + 1..offset + length + 1)
                .ok_or_else(|| anyhow::anyhow!("short DNS label"))?;
            out[size] = u8::try_from(length)?;
            for (dest, byte) in out[size + 1..][..length].iter_mut().zip(label) {
                *dest = byte.to_ascii_lowercase();
            }
            size += length + 1;
            offset += length + 1;
            if !jumped {
                *cursor = offset;
            }
            if length == 0 {
                return Ok(size);
            }
        }
    }
    bail!("too many DNS compression pointers")
}

pub(crate) fn parse_response(packet: &[u8]) -> Result<Response> {
    ensure!(packet.len() >= 12, "short DNS header");
    let flags = word(packet, 2)?;
    ensure!(
        flags & 0x8000 != 0 && flags & 0x7800 == 0,
        "not a standard DNS response"
    );
    ensure!(word(packet, 4)? == 1, "response must have one question");
    let mut cursor = 12;
    let mut question = [0; 255];
    let question_len = decode_name(packet, &mut cursor, &mut question)?;
    ensure!(
        word(packet, cursor)? == 1 && word(packet, cursor + 2)? == 1,
        "question is not A/IN"
    );
    cursor += 4;
    let answers = word(packet, 6)?;
    let records =
        usize::from(answers) + usize::from(word(packet, 8)?) + usize::from(word(packet, 10)?);
    let mut scratch = [0; 255];
    for _ in 0..records {
        decode_name(packet, &mut cursor, &mut scratch)?;
        let length = usize::from(word(packet, cursor + 8)?);
        cursor = cursor
            .checked_add(10 + length)
            .ok_or_else(|| anyhow::anyhow!("DNS length overflow"))?;
        ensure!(cursor <= packet.len(), "short DNS resource record");
    }
    ensure!(cursor == packet.len(), "trailing DNS bytes");
    Ok(Response {
        id: word(packet, 0)?,
        flags,
        answers,
        question,
        question_len,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_names_and_rd() -> Result<()> {
        let name = Name::parse("MiXeD.example.", true)?;
        assert_eq!(name.wire, b"\x05mixed\x07example\0");
        assert_eq!(name.query[2], 1);
        for invalid in ["", ".", "x..example", "a b.example", "é.example"] {
            assert!(Name::parse(invalid, false).is_err());
        }
        assert!(Name::parse(&format!("{}.example", "a".repeat(64)), false).is_err());
        assert!(Name::parse(&vec!["a".repeat(63); 4].join("."), false).is_err());
        Ok(())
    }

    #[test]
    fn rejects_truncation_and_pointer_cycles_without_panicking() -> Result<()> {
        let mut packet = Name::parse("a.example", false)?.query;
        packet[2] = 0x84;
        packet[7] = 1;
        packet.extend_from_slice(&[0xc0, 12, 0, 1, 0, 1, 0, 0, 0, 1, 0, 4, 127, 0, 0, 1]);
        assert_eq!(parse_response(&packet)?.answers, 1);
        for end in 0..packet.len() {
            assert!(parse_response(&packet[..end]).is_err());
        }
        packet[12] = 0xc0;
        packet[13] = 12;
        assert!(parse_response(&packet).is_err());
        Ok(())
    }
}
