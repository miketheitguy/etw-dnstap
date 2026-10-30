//! Frame Streams START, data, and STOP encoding for the TCP sender.
//!
//! Wire format reference: <https://github.com/farsightsec/fstrm/blob/master/fstrm/control.h>.

use std::io::{self, Write};

pub(crate) const CONTENT_TYPE: &[u8] = b"protobuf:dnstap.Dnstap";
const CONTROL_START: u32 = 2;
const CONTROL_STOP: u32 = 3;
const FIELD_CONTENT_TYPE: u32 = 1;
/// Resource limit, comfortably above a maximum DNS message plus metadata.
const MAX_FRAME_LENGTH: usize = 1_048_576;

/// A single dnstap stream. Call [`Self::finish`] to emit STOP and flush.
///
/// An I/O failure poisons the stream so later calls cannot append a misleading
/// STOP after a partial data frame. Dropping a writer does not hide flush errors
/// or attempt to finish the stream.
pub(crate) struct FrameStreamWriter<W: Write> {
    writer: W,
    failed: bool,
}

impl<W: Write> FrameStreamWriter<W> {
    /// Write START with the dnstap content type.
    pub(crate) fn new(mut writer: W) -> io::Result<Self> {
        let content_length = u32::try_from(CONTENT_TYPE.len())
            .map_err(|_| io::Error::other("content type length exceeds u32"))?;
        writer.write_all(&0_u32.to_be_bytes())?;
        writer.write_all(&(12 + content_length).to_be_bytes())?;
        writer.write_all(&CONTROL_START.to_be_bytes())?;
        writer.write_all(&FIELD_CONTENT_TYPE.to_be_bytes())?;
        writer.write_all(&content_length.to_be_bytes())?;
        writer.write_all(CONTENT_TYPE)?;
        Ok(Self {
            writer,
            failed: false,
        })
    }

    /// Write one nonempty protobuf payload, without protobuf length delimiting.
    pub(crate) fn write_frame(&mut self, payload: &[u8]) -> io::Result<()> {
        self.check_healthy()?;
        if payload.is_empty() || payload.len() > MAX_FRAME_LENGTH {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Frame Streams data length must be between 1 and 1048576 bytes",
            ));
        }
        let length = u32::try_from(payload.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame length exceeds u32"))?;
        self.failed = true;
        self.writer.write_all(&length.to_be_bytes())?;
        self.writer.write_all(payload)?;
        self.failed = false;
        Ok(())
    }

    /// Access the transport to set an operation deadline before writing.
    pub(crate) fn get_mut(&mut self) -> &mut W {
        &mut self.writer
    }

    /// Flush buffered output and poison the stream if flushing fails.
    pub(crate) fn flush(&mut self) -> io::Result<()> {
        self.check_healthy()?;
        self.failed = true;
        self.writer.flush()?;
        self.failed = false;
        Ok(())
    }

    /// Write STOP and flush buffered output, returning the underlying writer.
    pub(crate) fn finish(mut self) -> io::Result<W> {
        self.check_healthy()?;
        self.writer.write_all(&0_u32.to_be_bytes())?;
        self.writer.write_all(&4_u32.to_be_bytes())?;
        self.writer.write_all(&CONTROL_STOP.to_be_bytes())?;
        self.writer.flush()?;
        Ok(self.writer)
    }

    fn check_healthy(&self) -> io::Result<()> {
        if self.failed {
            return Err(io::Error::other(
                "Frame Streams writer failed during an earlier write",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_exact_start_data_and_stop_encoding() {
        let mut writer = FrameStreamWriter::new(Vec::new()).expect("START encodes");
        writer.write_frame(&[0x78, 1]).expect("data encodes");
        let bytes = writer.finish().expect("STOP and flush succeed");
        let mut expected = vec![0, 0, 0, 0, 0, 0, 0, 34, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 22];
        expected.extend_from_slice(b"protobuf:dnstap.Dnstap");
        expected.extend_from_slice(&[0, 0, 0, 2, 0x78, 1]);
        expected.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 3]);
        assert_eq!(bytes, expected);
    }

    #[test]
    fn rejects_zero_or_oversized_frames_without_changing_stream() {
        let mut writer = FrameStreamWriter::new(Vec::new()).expect("START encodes");
        let start_length = writer.writer.len();
        assert_eq!(
            writer
                .write_frame(&[])
                .expect_err("empty frame is invalid")
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            writer
                .write_frame(&vec![0; MAX_FRAME_LENGTH + 1])
                .expect_err("oversized frame is invalid")
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(writer.writer.len(), start_length);
        assert_eq!(
            writer.finish().expect("empty stream closes").len(),
            start_length + 12
        );
    }

    struct FailingWriter {
        remaining: usize,
    }

    impl Write for FailingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::other("injected write failure"));
            }
            let length = bytes.len().min(self.remaining).min(1);
            self.remaining -= length;
            Ok(length)
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("injected flush failure"))
        }
    }

    #[test]
    fn partial_write_failure_poisoning_prevents_appending_stop() {
        let mut writer = FrameStreamWriter::new(FailingWriter { remaining: 43 })
            .expect("START succeeds using short writes");
        assert!(writer.write_frame(&[1, 2, 3]).is_err());
        assert!(writer.failed);
        assert!(writer.write_frame(&[4]).is_err());
        assert!(writer.finish().is_err());
    }

    #[test]
    fn finish_reports_flush_failure() {
        let writer = FrameStreamWriter::new(FailingWriter { remaining: 1_000 })
            .expect("START succeeds using short writes");
        assert!(writer.finish().is_err());
    }

    #[test]
    fn periodic_flush_failure_poisons_the_stream() {
        let mut writer = FrameStreamWriter::new(FailingWriter { remaining: 1_000 })
            .expect("START succeeds using short writes");
        assert!(writer.flush().is_err());
        assert!(writer.failed);
        assert!(writer.write_frame(&[1]).is_err());
        assert!(writer.finish().is_err());
    }
}
