//! NUL-delimited framing used by `--remote-debugging-pipe`.

use bytes::{Bytes, BytesMut};

/// Splits a byte stream into NUL-terminated frames.
///
/// Feed it whatever the pipe returns with [`FrameDecoder::extend`], then drain
/// complete frames with [`FrameDecoder::next_frame`]. Frames can arrive split
/// across reads or several to a read; bytes after the last NUL stay buffered.
/// There is no size limit: screenshots can exceed 100 MB.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: BytesMut,
    /// How far `buf` has been scanned for a NUL without finding one, so a
    /// large frame arriving in many chunks is scanned only once.
    scanned: usize,
}

impl FrameDecoder {
    /// Creates an empty decoder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends bytes read from the pipe.
    pub fn extend(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// The internal buffer, for reading directly into it.
    pub(crate) fn buffer_mut(&mut self) -> &mut BytesMut {
        &mut self.buf
    }

    /// Returns the next complete frame (without its NUL), if there is one.
    pub fn next_frame(&mut self) -> Option<Bytes> {
        let Some(pos) = self.buf[self.scanned..].iter().position(|&b| b == 0) else {
            self.scanned = self.buf.len();
            return None;
        };
        let end = self.scanned + pos;
        let frame = self.buf.split_to(end).freeze();
        // Drop the NUL itself.
        let _ = self.buf.split_to(1);
        self.scanned = 0;
        Some(frame)
    }

    /// Bytes buffered after the last complete frame.
    pub fn pending(&self) -> usize {
        self.buf.len()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::FrameDecoder;

    fn drain(d: &mut FrameDecoder) -> Vec<Vec<u8>> {
        std::iter::from_fn(|| d.next_frame().map(|f| f.to_vec())).collect()
    }

    #[test]
    fn concatenated_frames() {
        let mut d = FrameDecoder::new();
        d.extend(b"{\"id\":1}\0{\"id\":2}\0{\"id\"");
        assert_eq!(
            drain(&mut d),
            [b"{\"id\":1}".to_vec(), b"{\"id\":2}".to_vec()]
        );
        assert_eq!(d.pending(), 5);
        d.extend(b":3}\0");
        assert_eq!(drain(&mut d), [b"{\"id\":3}".to_vec()]);
        assert_eq!(d.pending(), 0);
    }

    #[test]
    fn split_frame() {
        let mut d = FrameDecoder::new();
        for chunk in [&b"{\"a\""[..], b":", b"", b"1}"] {
            d.extend(chunk);
            assert!(d.next_frame().is_none());
        }
        d.extend(b"\0");
        assert_eq!(drain(&mut d), [b"{\"a\":1}".to_vec()]);
    }

    #[test]
    fn empty_frames() {
        let mut d = FrameDecoder::new();
        d.extend(b"\0\0x\0");
        assert_eq!(drain(&mut d), [vec![], vec![], b"x".to_vec()]);
    }

    proptest! {
        /// Any messages, cut into any chunks, decode back to the messages.
        #[test]
        fn chunking_does_not_matter(
            messages in prop::collection::vec(prop::collection::vec(1u8..=255, 0..200), 0..20),
            cuts in prop::collection::vec(any::<prop::sample::Index>(), 0..30),
        ) {
            let stream: Vec<u8> = messages.iter().flat_map(|m| m.iter().copied().chain([0])).collect();
            let mut cuts: Vec<usize> = cuts.iter().map(|i| i.index(stream.len() + 1)).collect();
            cuts.push(0);
            cuts.push(stream.len());
            cuts.sort_unstable();
            let mut d = FrameDecoder::new();
            let mut decoded = Vec::new();
            for w in cuts.windows(2) {
                d.extend(&stream[w[0]..w[1]]);
                decoded.extend(drain(&mut d));
            }
            prop_assert_eq!(decoded, messages);
            prop_assert_eq!(d.pending(), 0);
        }

        /// Arbitrary bytes never panic, and nothing after the last NUL is emitted.
        #[test]
        fn arbitrary_bytes(data in prop::collection::vec(any::<u8>(), 0..500)) {
            let mut d = FrameDecoder::new();
            d.extend(&data);
            let frames = drain(&mut d);
            let nuls = data.split(|&b| b == 0).count() - 1;
            prop_assert_eq!(frames.len(), nuls);
            let tail = data.iter().rev().take_while(|&&b| b != 0).count();
            prop_assert_eq!(d.pending(), tail);
        }
    }
}
