//! Routes datagrams on the single UDP mux socket to the owning session.
//!
//! STUN binding requests are routed by the SFU-side ICE ufrag they carry. Once a session
//! accepted traffic from a source address, all further DTLS/SRTP from that address takes the
//! address fast path.

use std::collections::HashMap;
use std::net::SocketAddr;

use super::stun;
use crate::domain::session::SessionId;

/// Address and ufrag routing tables.
#[derive(Debug, Default)]
pub struct Demux {
    by_ufrag: HashMap<String, SessionId>,
    by_addr: HashMap<SocketAddr, SessionId>,
}

impl Demux {
    /// Creates empty tables.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the SFU-side ufrag of a session.
    pub fn register(&mut self, local_ufrag: &str, session: SessionId) {
        self.by_ufrag.insert(local_ufrag.to_owned(), session);
    }

    /// Records that `addr` belongs to `session`.
    pub fn learn(&mut self, addr: SocketAddr, session: &SessionId) {
        self.by_addr.insert(addr, session.clone());
    }

    /// Forgets a session.
    pub fn remove(&mut self, session: &SessionId) {
        self.by_ufrag.retain(|_, s| s != session);
        self.by_addr.retain(|_, s| s != session);
    }

    /// Session that owns the datagram, and whether it was routed by ufrag (a STUN request).
    pub fn route(&self, source: SocketAddr, datagram: &[u8]) -> Option<(SessionId, bool)> {
        if let Some(session) = stun::local_ufrag(datagram).and_then(|u| self.by_ufrag.get(u)) {
            return Some((session.clone(), true));
        }
        self.by_addr.get(&source).map(|s| (s.clone(), false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::stun::binding_request;

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from(([10, 0, 0, 1], port))
    }

    #[test]
    fn stun_is_routed_by_ufrag() {
        let mut d = Demux::new();
        d.register("ufA", "a".into());
        d.register("ufB", "b".into());
        assert_eq!(
            d.route(addr(1), &binding_request("ufB:x")),
            Some(("b".into(), true))
        );
    }

    #[test]
    fn media_is_routed_by_learned_address() {
        let mut d = Demux::new();
        d.register("ufA", "a".into());
        assert_eq!(d.route(addr(1), &[0x80; 20]), None);
        d.learn(addr(1), &"a".into());
        assert_eq!(d.route(addr(1), &[0x80; 20]), Some(("a".into(), false)));
    }

    #[test]
    fn ufrag_wins_over_a_stale_address_mapping() {
        let mut d = Demux::new();
        d.register("ufB", "b".into());
        d.learn(addr(1), &"a".into());
        assert_eq!(
            d.route(addr(1), &binding_request("ufB:x")),
            Some(("b".into(), true))
        );
    }

    #[test]
    fn removal_forgets_both_tables() {
        let mut d = Demux::new();
        d.register("ufA", "a".into());
        d.learn(addr(1), &"a".into());
        d.remove(&"a".into());
        assert_eq!(d.route(addr(1), &binding_request("ufA:x")), None);
        assert_eq!(d.route(addr(1), &[0x80; 20]), None);
    }
}
