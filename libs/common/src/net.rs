//! Minimal Ethernet/ARP frame logic, shared as pure (host-tested) code that
//! defines the wire format the user-space `net` driver hand-writes/reads in
//! inline asm (like `virtio` consts, the driver can't call into kernel code).
//! ARP only — no IP/TCP. Big-endian on the wire.

/// Ethernet header length (dst[6] + src[6] + ethertype[2]).
pub const ETH_HDR_LEN: usize = 14;
/// EtherType for ARP.
pub const ETHERTYPE_ARP: u16 = 0x0806;
/// Total length of an ARP-over-Ethernet request/reply frame (14 + 28).
pub const ARP_FRAME_LEN: usize = 42;
/// ARP opcodes.
pub const ARP_REQUEST: u16 = 1;
pub const ARP_REPLY: u16 = 2;

/// Build an ARP **request** ("who-has `target_ip`, tell `src_ip`") as a full
/// Ethernet frame into `frame` (must be ≥ `ARP_FRAME_LEN`). Destination is the
/// broadcast MAC. Returns the frame length.
pub fn build_request(
    src_mac: &[u8; 6],
    src_ip: [u8; 4],
    target_ip: [u8; 4],
    frame: &mut [u8],
) -> usize {
    let f = &mut frame[..ARP_FRAME_LEN];
    // Ethernet header.
    f[0..6].copy_from_slice(&[0xff; 6]); // dst = broadcast
    f[6..12].copy_from_slice(src_mac); // src
    f[12..14].copy_from_slice(&ETHERTYPE_ARP.to_be_bytes());
    // ARP payload.
    f[14..16].copy_from_slice(&1u16.to_be_bytes()); // htype = Ethernet
    f[16..18].copy_from_slice(&0x0800u16.to_be_bytes()); // ptype = IPv4
    f[18] = 6; // hlen
    f[19] = 4; // plen
    f[20..22].copy_from_slice(&ARP_REQUEST.to_be_bytes());
    f[22..28].copy_from_slice(src_mac); // sender MAC
    f[28..32].copy_from_slice(&src_ip); // sender IP
    f[32..38].copy_from_slice(&[0u8; 6]); // target MAC = unknown
    f[38..42].copy_from_slice(&target_ip); // target IP
    ARP_FRAME_LEN
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

/// If `frame` is an ARP **reply** whose sender IP equals `want_ip`, return the
/// sender's MAC; otherwise `None`. Ignores non-ARP frames, wrong opcodes, and
/// replies for a different IP.
pub fn parse_reply(frame: &[u8], want_ip: [u8; 4]) -> Option<[u8; 6]> {
    if frame.len() < ARP_FRAME_LEN {
        return None;
    }
    if be16(&frame[12..14]) != ETHERTYPE_ARP {
        return None;
    }
    if be16(&frame[20..22]) != ARP_REPLY {
        return None;
    }
    if frame[28..32] != want_ip {
        return None;
    }
    let mut mac = [0u8; 6];
    mac.copy_from_slice(&frame[22..28]);
    Some(mac)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request_then_parse_fields() {
        let src_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let mut frame = [0u8; 64];
        let n = build_request(&src_mac, [10, 0, 2, 15], [10, 0, 2, 2], &mut frame);
        assert_eq!(n, ARP_FRAME_LEN);
        assert_eq!(&frame[0..6], &[0xff; 6], "broadcast dst");
        assert_eq!(&frame[6..12], &src_mac);
        assert_eq!(be16(&frame[12..14]), ETHERTYPE_ARP);
        assert_eq!(be16(&frame[20..22]), ARP_REQUEST);
        assert_eq!(&frame[28..32], &[10, 0, 2, 15]);
        assert_eq!(&frame[38..42], &[10, 0, 2, 2]);
    }

    #[test]
    fn parse_reply_returns_sender_mac() {
        // Synthesize a reply from 10.0.2.2 (gw) with a known MAC.
        let gw_mac = [0x52, 0x55, 0x0a, 0x00, 0x02, 0x02];
        let mut f = [0u8; ARP_FRAME_LEN];
        f[12..14].copy_from_slice(&ETHERTYPE_ARP.to_be_bytes());
        f[20..22].copy_from_slice(&ARP_REPLY.to_be_bytes());
        f[22..28].copy_from_slice(&gw_mac);
        f[28..32].copy_from_slice(&[10, 0, 2, 2]);
        assert_eq!(parse_reply(&f, [10, 0, 2, 2]), Some(gw_mac));
    }

    #[test]
    fn parse_reply_rejects_non_matching() {
        let mut f = [0u8; ARP_FRAME_LEN];
        f[12..14].copy_from_slice(&ETHERTYPE_ARP.to_be_bytes());
        f[20..22].copy_from_slice(&ARP_REPLY.to_be_bytes());
        f[28..32].copy_from_slice(&[10, 0, 2, 2]);
        assert!(parse_reply(&f, [10, 0, 2, 99]).is_none(), "wrong target ip");
        // wrong ethertype
        let mut g = f;
        g[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        assert!(parse_reply(&g, [10, 0, 2, 2]).is_none(), "not ARP");
        // a request, not a reply
        let mut h = f;
        h[20..22].copy_from_slice(&ARP_REQUEST.to_be_bytes());
        assert!(parse_reply(&h, [10, 0, 2, 2]).is_none(), "not a reply");
        // too short
        assert!(parse_reply(&f[..20], [10, 0, 2, 2]).is_none(), "truncated");
    }
}
