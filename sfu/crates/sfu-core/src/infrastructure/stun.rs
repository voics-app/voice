//! Minimal STUN inspection used to demultiplex the shared UDP port by ICE username.

const MAGIC_COOKIE: [u8; 4] = [0x21, 0x12, 0xA4, 0x42];
const HEADER_LEN: usize = 20;
const ATTR_USERNAME: u16 = 0x0006;

/// For a STUN binding request sent to the SFU, returns the SFU-side ufrag: the USERNAME
/// attribute is `<receiver ufrag>:<sender ufrag>` (RFC 8445, section 7.2.2).
pub fn local_ufrag(datagram: &[u8]) -> Option<&str> {
    if datagram.len() < HEADER_LEN || datagram[0] & 0xC0 != 0 || datagram[4..8] != MAGIC_COOKIE {
        return None;
    }
    let body_len = usize::from(u16::from_be_bytes([datagram[2], datagram[3]]));
    let body = datagram.get(HEADER_LEN..HEADER_LEN + body_len)?;

    let mut offset = 0;
    while offset + 4 <= body.len() {
        let attr_type = u16::from_be_bytes([body[offset], body[offset + 1]]);
        let attr_len = usize::from(u16::from_be_bytes([body[offset + 2], body[offset + 3]]));
        let value = body.get(offset + 4..offset + 4 + attr_len)?;
        if attr_type == ATTR_USERNAME {
            let username = std::str::from_utf8(value).ok()?;
            return username.split(':').next().filter(|u| !u.is_empty());
        }
        offset += 4 + attr_len.div_ceil(4) * 4;
    }
    None
}

#[cfg(test)]
pub(crate) fn binding_request(username: &str) -> Vec<u8> {
    let mut attr = Vec::new();
    attr.extend_from_slice(&ATTR_USERNAME.to_be_bytes());
    attr.extend_from_slice(&(username.len() as u16).to_be_bytes());
    attr.extend_from_slice(username.as_bytes());
    while attr.len() % 4 != 0 {
        attr.push(0);
    }
    let mut msg = vec![0x00, 0x01];
    msg.extend_from_slice(&(attr.len() as u16).to_be_bytes());
    msg.extend_from_slice(&MAGIC_COOKIE);
    msg.extend_from_slice(&[7; 12]);
    msg.extend_from_slice(&attr);
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_receiver_ufrag() {
        assert_eq!(local_ufrag(&binding_request("sfuA:cli1")), Some("sfuA"));
    }

    #[test]
    fn skips_other_attributes_with_padding() {
        let mut msg = binding_request("srv:cli");
        // Insert a 3-byte PRIORITY-like attribute (padded to 4) before USERNAME.
        let extra = [0x80, 0x29, 0x00, 0x03, 1, 2, 3, 0];
        msg.splice(20..20, extra);
        let len = (msg.len() - 20) as u16;
        msg[2..4].copy_from_slice(&len.to_be_bytes());
        assert_eq!(local_ufrag(&msg), Some("srv"));
    }

    #[test]
    fn non_stun_traffic_is_ignored() {
        assert_eq!(local_ufrag(&[0x80; 40]), None); // RTP
        assert_eq!(local_ufrag(&[0x16; 40]), None); // DTLS
        assert_eq!(local_ufrag(&[0; 10]), None);
    }

    #[test]
    fn truncated_attribute_is_rejected() {
        let mut msg = binding_request("srv:cli");
        msg.truncate(msg.len() - 4);
        assert_eq!(local_ufrag(&msg), None);
    }
}
