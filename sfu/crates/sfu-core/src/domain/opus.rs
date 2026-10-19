//! Opus payload inspection.

/// Largest payload that can only be a DTX / comfort-noise frame (TOC byte plus at most one
/// byte of frame data). Real speech frames are tens of bytes.
const MAX_DTX_PAYLOAD: usize = 2;

/// True for Opus DTX frames, which the SFU does not forward: subscribers' jitter buffers fill
/// silence themselves and the RTP rewriter keeps the sequence gap-free.
pub fn is_dtx_frame(payload: &[u8]) -> bool {
    payload.len() <= MAX_DTX_PAYLOAD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiny_payloads_are_dtx() {
        assert!(is_dtx_frame(&[]));
        assert!(is_dtx_frame(&[0xf8]));
        assert!(is_dtx_frame(&[0xf8, 0xff]));
    }

    #[test]
    fn speech_frames_are_not_dtx() {
        assert!(!is_dtx_frame(&[0x78; 40]));
        assert!(!is_dtx_frame(&[0xf8, 0xff, 0xfe]));
    }
}
