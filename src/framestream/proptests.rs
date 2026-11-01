//! Frame Streams writer properties: lossless under arbitrary write and read chunking.

use super::*;
use proptest::prelude::*;

#[derive(Debug, PartialEq, Eq)]
enum Item {
    Control(u32, Vec<u8>),
    Data(Vec<u8>),
}

/// Incremental receiver: accepts arbitrary byte chunks and yields whole frames.
#[derive(Default)]
struct Incremental {
    buffer: Vec<u8>,
    items: Vec<Item>,
}

impl Incremental {
    fn push(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
        loop {
            let Some((length, rest)) = self.buffer.split_first_chunk::<4>() else {
                return;
            };
            let length = u32::from_be_bytes(*length) as usize;
            if length > 0 {
                if rest.len() < length {
                    return;
                }
                self.items.push(Item::Data(rest[..length].to_vec()));
                self.buffer.drain(..4 + length);
                continue;
            }
            let Some((control_length, body)) = rest.split_first_chunk::<4>() else {
                return;
            };
            let control_length = u32::from_be_bytes(*control_length) as usize;
            if body.len() < control_length {
                return;
            }
            let kind = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
            self.items
                .push(Item::Control(kind, body[4..control_length].to_vec()));
            self.buffer.drain(..8 + control_length);
        }
    }
}

/// Accepts at most `limit` bytes per call, so `write_all` must loop.
struct Dribble {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for Dribble {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let count = data.len().min(self.limit);
        self.bytes.extend_from_slice(&data[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

proptest! {
    #[test]
    fn stream_round_trips_under_arbitrary_write_and_read_chunking(
        frames in prop::collection::vec(prop::collection::vec(any::<u8>(), 1..300), 0..20),
        write_limit in 1_usize..64,
        read_chunks in prop::collection::vec(1_usize..40, 1..16),
        flush_every in 1_usize..8,
    ) {
        let mut writer = FrameStreamWriter::new(Dribble { bytes: Vec::new(), limit: write_limit })
            .expect("START");
        for (index, frame) in frames.iter().enumerate() {
            writer.write_frame(frame).expect("data frame");
            if index % flush_every == 0 {
                writer.flush().expect("flush");
            }
        }
        let bytes = writer.finish().expect("STOP").bytes;

        let mut receiver = Incremental::default();
        let mut remaining = bytes.as_slice();
        for size in read_chunks.iter().cycle() {
            if remaining.is_empty() {
                break;
            }
            let (chunk, rest) = remaining.split_at((*size).min(remaining.len()));
            receiver.push(chunk);
            remaining = rest;
        }
        prop_assert!(receiver.buffer.is_empty(), "no partial frame left over");

        let mut expected = vec![Item::Control(2, {
            let mut field = FIELD_CONTENT_TYPE.to_be_bytes().to_vec();
            field.extend_from_slice(&u32::try_from(CONTENT_TYPE.len()).expect("len").to_be_bytes());
            field.extend_from_slice(CONTENT_TYPE);
            field
        })];
        expected.extend(frames.into_iter().map(Item::Data));
        expected.push(Item::Control(3, Vec::new()));
        prop_assert_eq!(receiver.items, expected);
    }

    #[test]
    fn rejected_frames_leave_the_stream_unchanged(
        size in prop_oneof![Just(0_usize), Just(MAX_FRAME_LENGTH + 1), MAX_FRAME_LENGTH + 2..MAX_FRAME_LENGTH + 100],
    ) {
        let mut writer = FrameStreamWriter::new(Vec::new()).expect("START");
        let before = writer.get_mut().len();
        let error = writer.write_frame(&vec![1; size]).expect_err("invalid size");
        prop_assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        prop_assert_eq!(writer.get_mut().len(), before);
    }
}

#[test]
fn maximum_and_one_byte_frames_round_trip() {
    for size in [1, MAX_FRAME_LENGTH] {
        let frame = vec![0xc3; size];
        let mut writer = FrameStreamWriter::new(Vec::new()).expect("START");
        writer.write_frame(&frame).expect("in-range frame");
        let bytes = writer.finish().expect("STOP");
        let mut receiver = Incremental::default();
        receiver.push(&bytes);
        assert_eq!(receiver.items.len(), 3);
        assert_eq!(receiver.items[1], Item::Data(frame));
    }
}
