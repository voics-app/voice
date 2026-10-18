//! VP9 RTP payload descriptor (RFC 9628, section 4.2), parsed far enough to route SVC layers.

/// The routing-relevant fields of a VP9 payload descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vp9Descriptor {
    /// `P`: the picture references earlier pictures (inter picture).
    pub inter_picture: bool,
    /// `B`: first packet of a layer frame.
    pub begins_frame: bool,
    /// `E`: last packet of a layer frame.
    pub ends_frame: bool,
    /// `U`: temporal up-switch point.
    pub switching_up: bool,
    /// Spatial layer id (0 when layer indices are absent).
    pub spatial_id: u8,
    /// Temporal layer id (0 when layer indices are absent).
    pub temporal_id: u8,
}

impl Vp9Descriptor {
    /// Parses the descriptor at the start of an RTP payload.
    pub fn parse(payload: &[u8]) -> Option<Self> {
        let first = *payload.first()?;
        let has_picture_id = first & 0x80 != 0;
        let inter_picture = first & 0x40 != 0;
        let has_layer_indices = first & 0x20 != 0;
        let flexible = first & 0x10 != 0;
        let begins_frame = first & 0x08 != 0;
        let ends_frame = first & 0x04 != 0;

        let mut offset = 1;
        if has_picture_id {
            let pid = *payload.get(offset)?;
            offset += if pid & 0x80 != 0 { 2 } else { 1 };
        }

        let (temporal_id, switching_up, spatial_id) = if has_layer_indices {
            let layer = *payload.get(offset)?;
            (layer >> 5, layer & 0x10 != 0, (layer >> 1) & 0x07)
        } else {
            (0, false, 0)
        };

        // Remaining fields (TL0PICIDX, P_DIFF, scalability structure) are not needed for routing,
        // but a descriptor that claims layer indices must contain them.
        if has_layer_indices && !flexible {
            payload.get(offset + 1)?;
        }

        Some(Self {
            inter_picture,
            begins_frame,
            ends_frame,
            switching_up,
            spatial_id,
            temporal_id,
        })
    }

    /// True for the first packet of the base spatial layer of a key picture.
    pub fn starts_keyframe(&self) -> bool {
        !self.inter_picture && self.begins_frame && self.spatial_id == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_non_flexible_descriptor_with_layers() {
        // I=1 P=0 L=1 F=0 B=1 E=0 | M=1 pid (2 bytes) | TID=2 U=1 SID=1 D=0 | TL0PICIDX
        let d = Vp9Descriptor::parse(&[0xA8, 0x81, 0x23, 0b0101_0010, 0x07]).unwrap();
        assert!(!d.inter_picture);
        assert!(d.begins_frame);
        assert!(!d.ends_frame);
        assert!(d.switching_up);
        assert_eq!(d.temporal_id, 2);
        assert_eq!(d.spatial_id, 1);
        assert!(
            !d.starts_keyframe(),
            "spatial layer 1 is not the keyframe start"
        );
    }

    #[test]
    fn parses_flexible_descriptor() {
        // I=1 P=1 L=1 F=1 B=0 E=1 | short pid | TID=0 SID=2 | P_DIFF
        let d = Vp9Descriptor::parse(&[0xF4, 0x05, 0b0000_0100, 0x02]).unwrap();
        assert!(d.inter_picture);
        assert!(d.ends_frame);
        assert_eq!(d.spatial_id, 2);
        assert_eq!(d.temporal_id, 0);
    }

    #[test]
    fn keyframe_start_without_layer_indices() {
        let d = Vp9Descriptor::parse(&[0x08]).unwrap();
        assert!(d.starts_keyframe());
    }

    #[test]
    fn truncated_descriptors_are_rejected() {
        assert_eq!(Vp9Descriptor::parse(&[]), None);
        assert_eq!(Vp9Descriptor::parse(&[0x80]), None);
        assert_eq!(Vp9Descriptor::parse(&[0xA0, 0x01, 0x00]), None);
    }
}
