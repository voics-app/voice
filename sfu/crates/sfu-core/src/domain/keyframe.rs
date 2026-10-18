//! Detects whether an RTP payload starts a keyframe, per codec payload format.
//!
//! The SFU may only switch a subscriber to another simulcast layer at a keyframe, otherwise the
//! decoder would receive inter frames that reference pictures it never saw.

use super::codec::VideoCodec;
use super::vp9_descriptor::Vp9Descriptor;

/// Returns true when `payload` is the first packet of a keyframe.
pub fn starts_keyframe(codec: VideoCodec, payload: &[u8]) -> bool {
    match codec {
        VideoCodec::Vp8 => vp8_starts_keyframe(payload),
        VideoCodec::Vp9 => Vp9Descriptor::parse(payload).is_some_and(|d| d.starts_keyframe()),
        VideoCodec::H264 => h264_starts_keyframe(payload),
        VideoCodec::Av1 => av1_starts_keyframe(payload),
    }
}

/// RFC 7741 payload descriptor followed by the VP8 payload header.
fn vp8_starts_keyframe(payload: &[u8]) -> bool {
    let Some(&first) = payload.first() else {
        return false;
    };
    let extended = first & 0x80 != 0;
    let start_of_partition = first & 0x10 != 0;
    let partition_id = first & 0x07;
    if !start_of_partition || partition_id != 0 {
        return false;
    }

    let mut offset = 1;
    if extended {
        let Some(&ext) = payload.get(offset) else {
            return false;
        };
        offset += 1;
        let has_picture_id = ext & 0x80 != 0;
        let has_tl0_pic_idx = ext & 0x40 != 0;
        let has_tid_or_key_idx = ext & 0x30 != 0;
        if has_picture_id {
            let Some(&pid) = payload.get(offset) else {
                return false;
            };
            offset += if pid & 0x80 != 0 { 2 } else { 1 };
        }
        if has_tl0_pic_idx {
            offset += 1;
        }
        if has_tid_or_key_idx {
            offset += 1;
        }
    }

    // VP8 frame tag: bit 0 is the inverse key frame flag.
    payload.get(offset).is_some_and(|tag| tag & 0x01 == 0)
}

const H264_IDR: u8 = 5;
const H264_SPS: u8 = 7;
const H264_STAP_A: u8 = 24;
const H264_FU_A: u8 = 28;

/// RFC 6184: single NAL, STAP-A aggregation or the first FU-A fragment of an IDR/SPS.
fn h264_starts_keyframe(payload: &[u8]) -> bool {
    let Some(&first) = payload.first() else {
        return false;
    };
    let is_key_nal = |nal_type: u8| nal_type == H264_IDR || nal_type == H264_SPS;
    match first & 0x1F {
        H264_STAP_A => {
            let mut offset = 1;
            while offset + 2 < payload.len() {
                let size = usize::from(u16::from_be_bytes([payload[offset], payload[offset + 1]]));
                let Some(&header) = payload.get(offset + 2) else {
                    return false;
                };
                if is_key_nal(header & 0x1F) {
                    return true;
                }
                offset += 2 + size;
            }
            false
        }
        H264_FU_A => payload
            .get(1)
            .is_some_and(|fu| fu & 0x80 != 0 && is_key_nal(fu & 0x1F)),
        nal_type => is_key_nal(nal_type),
    }
}

/// AV1 RTP aggregation header: `N` marks the first packet of a coded video sequence and `Z`
/// marks a continuation of an OBU from the previous packet.
fn av1_starts_keyframe(payload: &[u8]) -> bool {
    payload
        .first()
        .is_some_and(|h| h & 0x08 != 0 && h & 0x80 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vp8_minimal_descriptor_keyframe() {
        // S=1, PID=0; frame tag P bit = 0.
        assert!(starts_keyframe(VideoCodec::Vp8, &[0x10, 0x00, 0x9d]));
        // Inter frame.
        assert!(!starts_keyframe(VideoCodec::Vp8, &[0x10, 0x01, 0x9d]));
        // Not the start of the partition.
        assert!(!starts_keyframe(VideoCodec::Vp8, &[0x00, 0x00]));
    }

    #[test]
    fn vp8_extended_descriptor_with_long_picture_id() {
        // X=1,S=1 | I=1,L=1,T=1 | M=1 picture id (2 bytes) | TL0PICIDX | TID/KEYIDX | tag
        let key = [0x90, 0xE0, 0x80, 0x01, 0x05, 0x20, 0x10];
        assert!(starts_keyframe(VideoCodec::Vp8, &key));
        let inter = [0x90, 0xE0, 0x80, 0x01, 0x05, 0x20, 0x11];
        assert!(!starts_keyframe(VideoCodec::Vp8, &inter));
    }

    #[test]
    fn vp8_truncated_payload_is_not_a_keyframe() {
        assert!(!starts_keyframe(VideoCodec::Vp8, &[]));
        assert!(!starts_keyframe(VideoCodec::Vp8, &[0x90]));
        assert!(!starts_keyframe(VideoCodec::Vp8, &[0x90, 0x80]));
    }

    #[test]
    fn h264_single_nal_types() {
        assert!(starts_keyframe(VideoCodec::H264, &[0x65, 0x88])); // IDR
        assert!(starts_keyframe(VideoCodec::H264, &[0x67, 0x42])); // SPS
        assert!(!starts_keyframe(VideoCodec::H264, &[0x41, 0x9a])); // non-IDR slice
    }

    #[test]
    fn h264_stap_a_containing_sps() {
        // STAP-A | size=2 | SPS hdr + 1 byte | size=2 | PPS hdr + 1 byte
        let stap = [0x78, 0x00, 0x02, 0x67, 0x42, 0x00, 0x02, 0x68, 0xce];
        assert!(starts_keyframe(VideoCodec::H264, &stap));
        let no_key = [0x78, 0x00, 0x02, 0x41, 0x00, 0x00, 0x02, 0x68, 0xce];
        assert!(!starts_keyframe(VideoCodec::H264, &no_key));
    }

    #[test]
    fn h264_fu_a_only_first_fragment_counts() {
        assert!(starts_keyframe(VideoCodec::H264, &[0x7c, 0x85, 0x00]));
        assert!(!starts_keyframe(VideoCodec::H264, &[0x7c, 0x05, 0x00])); // middle fragment
        assert!(!starts_keyframe(VideoCodec::H264, &[0x7c, 0x81, 0x00])); // non-IDR start
    }

    #[test]
    fn av1_new_sequence_flag() {
        assert!(starts_keyframe(VideoCodec::Av1, &[0x18]));
        assert!(!starts_keyframe(VideoCodec::Av1, &[0x10]));
        assert!(!starts_keyframe(VideoCodec::Av1, &[0x88])); // continuation
    }

    #[test]
    fn vp9_keyframe_delegates_to_descriptor() {
        // I=0 P=0 L=0 F=0 B=1 E=1 V=0
        assert!(starts_keyframe(VideoCodec::Vp9, &[0x0C, 0x00]));
        // P=1 (inter picture)
        assert!(!starts_keyframe(VideoCodec::Vp9, &[0x4C, 0x00]));
    }
}
