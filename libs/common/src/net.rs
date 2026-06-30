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

/// IPv4 — the smallest header needed to carry UDP. Big-endian on the wire.
pub mod ipv4 {
    pub const IPV4_HDR_LEN: usize = 20;
    pub const PROTO_UDP: u8 = 17;
    pub const ETHERTYPE_IPV4: u16 = 0x0800;

    /// One's-complement checksum (RFC 1071) over `bytes`, as the IPv4 header
    /// uses. A header whose checksum field already holds the result verifies to 0.
    pub fn checksum(bytes: &[u8]) -> u16 {
        let mut sum: u32 = 0;
        let mut i = 0;
        while i + 1 < bytes.len() {
            sum += u16::from_be_bytes([bytes[i], bytes[i + 1]]) as u32;
            i += 2;
        }
        if i < bytes.len() {
            sum += (bytes[i] as u32) << 8; // odd trailing byte, high-padded
        }
        while sum >> 16 != 0 {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        !(sum as u16)
    }

    /// Write a 20-byte IPv4 header carrying `payload_len` bytes of `proto` into
    /// `out` (TTL 64, no fragmentation, header checksum computed). Returns 20.
    pub fn build_header(
        src_ip: [u8; 4],
        dst_ip: [u8; 4],
        proto: u8,
        payload_len: usize,
        ident: u16,
        out: &mut [u8],
    ) -> usize {
        let h = &mut out[..IPV4_HDR_LEN];
        h[0] = 0x45; // version 4, IHL 5 (20 bytes)
        h[1] = 0; // DSCP/ECN
        let total = (IPV4_HDR_LEN + payload_len) as u16;
        h[2..4].copy_from_slice(&total.to_be_bytes());
        h[4..6].copy_from_slice(&ident.to_be_bytes());
        h[6..8].copy_from_slice(&0u16.to_be_bytes()); // flags + fragment offset
        h[8] = 64; // TTL
        h[9] = proto;
        h[10..12].copy_from_slice(&0u16.to_be_bytes()); // checksum: zero, then fill
        h[12..16].copy_from_slice(&src_ip);
        h[16..20].copy_from_slice(&dst_ip);
        let csum = checksum(h);
        h[10..12].copy_from_slice(&csum.to_be_bytes());
        IPV4_HDR_LEN
    }
}

/// UDP over IPv4 over Ethernet. Builds a full frame; parses incoming datagrams
/// by destination port. UDP checksum is 0 on send (valid for IPv4).
pub mod udp {
    use super::ipv4;
    pub const UDP_HDR_LEN: usize = 8;

    /// Assemble Ethernet + IPv4 + UDP around `payload` into `frame`. Returns the
    /// total frame length (14 + 20 + 8 + payload).
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        src_mac: &[u8; 6],
        dst_mac: &[u8; 6],
        src_ip: [u8; 4],
        dst_ip: [u8; 4],
        src_port: u16,
        dst_port: u16,
        ident: u16,
        payload: &[u8],
        frame: &mut [u8],
    ) -> usize {
        // Ethernet header.
        frame[0..6].copy_from_slice(dst_mac);
        frame[6..12].copy_from_slice(src_mac);
        frame[12..14].copy_from_slice(&ipv4::ETHERTYPE_IPV4.to_be_bytes());
        // IPv4 header (covers the UDP header + payload).
        let udp_len = UDP_HDR_LEN + payload.len();
        let ip = super::ETH_HDR_LEN;
        ipv4::build_header(src_ip, dst_ip, ipv4::PROTO_UDP, udp_len, ident, &mut frame[ip..ip + ipv4::IPV4_HDR_LEN]);
        // UDP header + payload.
        let u = ip + ipv4::IPV4_HDR_LEN;
        frame[u..u + 2].copy_from_slice(&src_port.to_be_bytes());
        frame[u + 2..u + 4].copy_from_slice(&dst_port.to_be_bytes());
        frame[u + 4..u + 6].copy_from_slice(&(udp_len as u16).to_be_bytes());
        frame[u + 6..u + 8].copy_from_slice(&0u16.to_be_bytes()); // checksum 0 (optional for IPv4)
        frame[u + 8..u + 8 + payload.len()].copy_from_slice(payload);
        u + 8 + payload.len()
    }

    /// If `frame` is an IPv4/UDP datagram addressed to `want_dst_port`, return its
    /// UDP payload. Lenient on checksums (we demux by port). `None` otherwise.
    pub fn parse(frame: &[u8], want_dst_port: u16) -> Option<&[u8]> {
        let eth = super::ETH_HDR_LEN;
        if frame.len() < eth + ipv4::IPV4_HDR_LEN + UDP_HDR_LEN {
            return None;
        }
        if super::be16(&frame[12..14]) != ipv4::ETHERTYPE_IPV4 {
            return None;
        }
        let ihl = (frame[eth] & 0x0f) as usize * 4;
        if ihl < ipv4::IPV4_HDR_LEN || frame.len() < eth + ihl + UDP_HDR_LEN {
            return None;
        }
        if frame[eth + 9] != ipv4::PROTO_UDP {
            return None;
        }
        let u = eth + ihl;
        if super::be16(&frame[u + 2..u + 4]) != want_dst_port {
            return None;
        }
        let ulen = super::be16(&frame[u + 4..u + 6]) as usize;
        if ulen < UDP_HDR_LEN || u + ulen > frame.len() {
            return None;
        }
        Some(&frame[u + UDP_HDR_LEN..u + ulen])
    }
}

/// Minimal DHCP (over BOOTP): build a DISCOVER, parse an OFFER's offered address.
pub mod dhcp {
    pub const CLIENT_PORT: u16 = 68;
    pub const SERVER_PORT: u16 = 67;
    const MAGIC: [u8; 4] = [0x63, 0x82, 0x53, 0x63];
    const OP_REQUEST: u8 = 1;
    const OP_REPLY: u8 = 2;
    const MSG_DISCOVER: u8 = 1;
    const MSG_OFFER: u8 = 2;
    const MSG_REQUEST: u8 = 3;
    const MSG_ACK: u8 = 5;
    /// BOOTP fixed area (op..file) before the magic cookie.
    const BOOTP_FIXED: usize = 236;
    /// DISCOVER payload: fixed area + cookie(4) + option 53 (3) + end (1).
    pub const DISCOVER_LEN: usize = BOOTP_FIXED + 4 + 3 + 1;

    /// What a DHCPOFFER tells us: the offered address and the server identifier
    /// (option 54) that a REQUEST must echo so the right server commits the lease.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Offer {
        pub yiaddr: [u8; 4],
        pub server_id: [u8; 4],
    }

    /// Build a DHCPDISCOVER BOOTP payload into `out` (>= DISCOVER_LEN). The
    /// broadcast flag is set so the OFFER is broadcast back (we have no IP yet).
    /// Returns the payload length.
    pub fn build_discover(xid: u32, client_mac: &[u8; 6], out: &mut [u8]) -> usize {
        let p = &mut out[..DISCOVER_LEN];
        for b in p.iter_mut() {
            *b = 0;
        }
        p[0] = OP_REQUEST;
        p[1] = 1; // htype = Ethernet
        p[2] = 6; // hlen
        p[4..8].copy_from_slice(&xid.to_be_bytes());
        p[10..12].copy_from_slice(&0x8000u16.to_be_bytes()); // flags: broadcast
        p[28..34].copy_from_slice(client_mac); // chaddr (first 6 of 16)
        p[236..240].copy_from_slice(&MAGIC);
        p[240] = 53; // option: DHCP message type
        p[241] = 1;
        p[242] = MSG_DISCOVER;
        p[243] = 255; // end
        DISCOVER_LEN
    }

    /// REQUEST payload: fixed area + cookie(4) + opt 53 (3) + opt 50 (6) +
    /// opt 54 (6) + end (1).
    pub const REQUEST_LEN: usize = BOOTP_FIXED + 4 + 3 + 6 + 6 + 1;

    /// Build a DHCPREQUEST into `out` (>= REQUEST_LEN): broadcast like DISCOVER,
    /// message type REQUEST, with option 50 (requested IP = the offer's `yiaddr`)
    /// and option 54 (server id from the offer). Returns the payload length.
    pub fn build_request(
        xid: u32,
        client_mac: &[u8; 6],
        requested_ip: [u8; 4],
        server_id: [u8; 4],
        out: &mut [u8],
    ) -> usize {
        let p = &mut out[..REQUEST_LEN];
        for b in p.iter_mut() {
            *b = 0;
        }
        p[0] = OP_REQUEST;
        p[1] = 1; // htype = Ethernet
        p[2] = 6; // hlen
        p[4..8].copy_from_slice(&xid.to_be_bytes());
        p[10..12].copy_from_slice(&0x8000u16.to_be_bytes()); // flags: broadcast
        p[28..34].copy_from_slice(client_mac); // chaddr
        p[236..240].copy_from_slice(&MAGIC);
        let mut o = 240;
        p[o] = 53; // DHCP message type
        p[o + 1] = 1;
        p[o + 2] = MSG_REQUEST;
        o += 3;
        p[o] = 50; // requested IP address
        p[o + 1] = 4;
        p[o + 2..o + 6].copy_from_slice(&requested_ip);
        o += 6;
        p[o] = 54; // server identifier
        p[o + 1] = 4;
        p[o + 2..o + 6].copy_from_slice(&server_id);
        o += 6;
        p[o] = 255; // end
        REQUEST_LEN
    }

    /// If `payload` is a DHCPACK (BOOTREPLY, our `xid`, magic cookie, message type
    /// ACK), return the confirmed address (`yiaddr`). `None` otherwise.
    pub fn parse_ack(payload: &[u8], xid: u32) -> Option<[u8; 4]> {
        if !is_reply(payload, xid, MSG_ACK) {
            return None;
        }
        let mut ip = [0u8; 4];
        ip.copy_from_slice(&payload[16..20]);
        Some(ip)
    }

    /// If `payload` is a DHCPOFFER (BOOTREPLY, our `xid`, magic cookie, message
    /// type OFFER), return the offered address and server id. `None` otherwise.
    pub fn parse_offer(payload: &[u8], xid: u32) -> Option<Offer> {
        if !is_reply(payload, xid, MSG_OFFER) {
            return None;
        }
        let mut yiaddr = [0u8; 4];
        yiaddr.copy_from_slice(&payload[16..20]);
        let mut server_id = [0u8; 4];
        if let Some(s) = option(&payload[240..], 54) {
            if s.len() >= 4 {
                server_id.copy_from_slice(&s[..4]);
            }
        }
        Some(Offer { yiaddr, server_id })
    }

    /// Common BOOTREPLY guard: length, op = reply, our `xid`, magic cookie, and
    /// DHCP message type (option 53) == `msg_type`.
    fn is_reply(payload: &[u8], xid: u32, msg_type: u8) -> bool {
        payload.len() >= 240
            && payload[0] == OP_REPLY
            && payload[4..8] == xid.to_be_bytes()
            && payload[236..240] == MAGIC
            && option(&payload[240..], 53).and_then(|v| v.first()).copied() == Some(msg_type)
    }

    /// Walk the TLV option area for `code`, returning its value bytes. `0` = pad,
    /// `255` = end; every other option is `code, len, value…`.
    fn option(opts: &[u8], code: u8) -> Option<&[u8]> {
        let mut i = 0;
        while i < opts.len() {
            match opts[i] {
                0 => i += 1,
                255 => return None,
                c => {
                    if i + 1 >= opts.len() {
                        return None;
                    }
                    let len = opts[i + 1] as usize;
                    if i + 2 + len > opts.len() {
                        return None;
                    }
                    if c == code {
                        return Some(&opts[i + 2..i + 2 + len]);
                    }
                    i += 2 + len;
                }
            }
        }
        None
    }
}

/// ICMP echo (ping) over IPv4. Reuses `ipv4::build_header` (protocol 1) and
/// `ipv4::checksum` (ICMP uses the same RFC 1071 one's-complement checksum).
pub mod icmp {
    use super::ipv4;
    pub const PROTO_ICMP: u8 = 1;
    pub const ICMP_ECHO_REQUEST: u8 = 8;
    pub const ICMP_ECHO_REPLY: u8 = 0;
    /// ICMP echo header: type(1) + code(1) + checksum(2) + ident(2) + seq(2).
    pub const ICMP_HDR_LEN: usize = 8;

    /// Assemble Ethernet + IPv4 + ICMP Echo Request around `payload` into `frame`.
    /// Returns the total frame length.
    #[allow(clippy::too_many_arguments)]
    pub fn build_echo_request(
        src_mac: &[u8; 6],
        dst_mac: &[u8; 6],
        src_ip: [u8; 4],
        dst_ip: [u8; 4],
        ident: u16,
        seq: u16,
        payload: &[u8],
        frame: &mut [u8],
    ) -> usize {
        // Ethernet header.
        frame[0..6].copy_from_slice(dst_mac);
        frame[6..12].copy_from_slice(src_mac);
        frame[12..14].copy_from_slice(&ipv4::ETHERTYPE_IPV4.to_be_bytes());
        // IPv4 header (covers the ICMP message).
        let icmp_len = ICMP_HDR_LEN + payload.len();
        let ip = super::ETH_HDR_LEN;
        ipv4::build_header(src_ip, dst_ip, PROTO_ICMP, icmp_len, ident, &mut frame[ip..ip + ipv4::IPV4_HDR_LEN]);
        // ICMP echo request.
        let c = ip + ipv4::IPV4_HDR_LEN;
        frame[c] = ICMP_ECHO_REQUEST;
        frame[c + 1] = 0; // code
        frame[c + 2..c + 4].copy_from_slice(&0u16.to_be_bytes()); // checksum: zero, then fill
        frame[c + 4..c + 6].copy_from_slice(&ident.to_be_bytes());
        frame[c + 6..c + 8].copy_from_slice(&seq.to_be_bytes());
        frame[c + 8..c + 8 + payload.len()].copy_from_slice(payload);
        let csum = ipv4::checksum(&frame[c..c + icmp_len]);
        frame[c + 2..c + 4].copy_from_slice(&csum.to_be_bytes());
        c + icmp_len
    }

    /// True iff `frame` is an IPv4/ICMP **Echo Reply** with the matching identifier
    /// and sequence. Lenient on the reply's checksum (we trust the kernel demux).
    pub fn parse_echo_reply(frame: &[u8], ident: u16, seq: u16) -> bool {
        let eth = super::ETH_HDR_LEN;
        if frame.len() < eth + ipv4::IPV4_HDR_LEN + ICMP_HDR_LEN {
            return false;
        }
        if super::be16(&frame[12..14]) != ipv4::ETHERTYPE_IPV4 {
            return false;
        }
        let ihl = (frame[eth] & 0x0f) as usize * 4;
        if ihl < ipv4::IPV4_HDR_LEN || frame.len() < eth + ihl + ICMP_HDR_LEN {
            return false;
        }
        if frame[eth + 9] != PROTO_ICMP {
            return false;
        }
        let c = eth + ihl;
        frame[c] == ICMP_ECHO_REPLY
            && super::be16(&frame[c + 4..c + 6]) == ident
            && super::be16(&frame[c + 6..c + 8]) == seq
    }
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

    #[test]
    fn ipv4_checksum_canonical_example() {
        // Canonical IPv4 header (Wikipedia) WITH its real checksum 0xb861 in
        // place re-checksums to 0; with the checksum field zeroed it yields 0xb861.
        let full = [
            0x45u8, 0x00, 0x00, 0x73, 0x00, 0x00, 0x40, 0x00, 0x40, 0x11, 0xb8, 0x61,
            0xc0, 0xa8, 0x00, 0x01, 0xc0, 0xa8, 0x00, 0xc7,
        ];
        assert_eq!(ipv4::checksum(&full), 0, "valid header verifies to 0");
        let mut zeroed = full;
        zeroed[10] = 0;
        zeroed[11] = 0;
        assert_eq!(ipv4::checksum(&zeroed), 0xb861, "canonical checksum");
    }

    #[test]
    fn ipv4_build_header_verifies_and_fields() {
        let mut out = [0u8; 20];
        let n = ipv4::build_header([10, 0, 2, 15], [255, 255, 255, 255], ipv4::PROTO_UDP, 8, 0x1234, &mut out);
        assert_eq!(n, ipv4::IPV4_HDR_LEN);
        assert_eq!(ipv4::checksum(&out), 0, "built header self-verifies");
        assert_eq!(out[0], 0x45, "version 4, IHL 5");
        assert_eq!(out[9], ipv4::PROTO_UDP);
        assert_eq!(&out[12..16], &[10, 0, 2, 15]);
        assert_eq!(&out[16..20], &[255, 255, 255, 255]);
    }

    #[test]
    fn udp_build_then_parse_roundtrip() {
        let src_mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
        let dst_mac = [0xffu8; 6];
        let payload = [0xde_u8, 0xad, 0xbe, 0xef];
        let mut frame = [0u8; 128];
        let n = udp::build(&src_mac, &dst_mac, [0, 0, 0, 0], [255, 255, 255, 255], 68, 67, 0x1234, &payload, &mut frame);
        // IPv4 header (bytes 14..34) self-verifies.
        assert_eq!(ipv4::checksum(&frame[14..34]), 0);
        // Wrong port -> None; right port -> the payload back.
        assert!(udp::parse(&frame[..n], 53).is_none());
        assert_eq!(udp::parse(&frame[..n], 67), Some(&payload[..]));
    }

    #[test]
    fn udp_parse_rejects_non_udp() {
        let mut frame = [0u8; 64];
        frame[12..14].copy_from_slice(&0x0806u16.to_be_bytes()); // ARP ethertype
        assert!(udp::parse(&frame, 67).is_none());
    }

    #[test]
    fn dhcp_discover_then_offer_roundtrip() {
        let mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
        let xid = 0x1234_5678u32;
        let mut disc = [0u8; dhcp::DISCOVER_LEN];
        let n = dhcp::build_discover(xid, &mac, &mut disc);
        assert_eq!(n, dhcp::DISCOVER_LEN);
        assert_eq!(disc[0], 1, "BOOTREQUEST");
        assert_eq!(&disc[236..240], &[0x63, 0x82, 0x53, 0x63], "magic cookie");
        assert_eq!(&disc[28..34], &mac, "chaddr");
        // Synthesize the OFFER the server would send back.
        let mut offer = disc;
        offer[0] = 2; // BOOTREPLY
        offer[16..20].copy_from_slice(&[10, 0, 2, 15]); // yiaddr
        offer[242] = 2; // option 53 value = OFFER
        assert_eq!(dhcp::parse_offer(&offer, xid).map(|o| o.yiaddr), Some([10, 0, 2, 15]));
    }

    #[test]
    fn dhcp_parse_offer_rejects_mismatches() {
        let mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
        let xid = 0xaabb_ccddu32;
        let mut offer = [0u8; dhcp::DISCOVER_LEN];
        dhcp::build_discover(xid, &mac, &mut offer);
        offer[0] = 2;
        offer[16..20].copy_from_slice(&[10, 0, 2, 15]);
        offer[242] = 2;
        assert!(dhcp::parse_offer(&offer, 0x9999_9999).is_none(), "wrong xid");
        let mut req = offer;
        req[0] = 1; // BOOTREQUEST, not reply
        assert!(dhcp::parse_offer(&req, xid).is_none(), "not a reply");
        let mut not_offer = offer;
        not_offer[242] = 1; // msg type DISCOVER, not OFFER
        assert!(dhcp::parse_offer(&not_offer, xid).is_none(), "not an offer");
    }

    #[test]
    fn dhcp_request_build_then_reparse() {
        let mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
        let xid = 0x1234_5678u32;
        let mut req = [0u8; dhcp::REQUEST_LEN];
        let n = dhcp::build_request(xid, &mac, [10, 0, 2, 15], [10, 0, 2, 2], &mut req);
        assert_eq!(n, dhcp::REQUEST_LEN);
        assert_eq!(req[0], 1, "BOOTREQUEST");
        assert_eq!(&req[4..8], &xid.to_be_bytes());
        assert_eq!(&req[236..240], &[0x63, 0x82, 0x53, 0x63], "magic cookie");
        // Options: 53=REQUEST(3), 50=requested IP, 54=server id, end.
        assert_eq!(&req[240..243], &[53, 1, 3], "msg type = REQUEST");
        assert_eq!(&req[243..249], &[50, 4, 10, 0, 2, 15], "requested IP");
        assert_eq!(&req[249..255], &[54, 4, 10, 0, 2, 2], "server id");
        assert_eq!(req[255], 255, "end");
    }

    #[test]
    fn dhcp_parse_ack_returns_address() {
        let mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
        let xid = 0x1234_5678u32;
        // Start from a REQUEST, turn it into the ACK the server would send.
        let mut ack = [0u8; dhcp::REQUEST_LEN];
        dhcp::build_request(xid, &mac, [10, 0, 2, 15], [10, 0, 2, 2], &mut ack);
        ack[0] = 2; // BOOTREPLY
        ack[16..20].copy_from_slice(&[10, 0, 2, 15]); // yiaddr
        ack[242] = 5; // option 53 value = ACK
        assert_eq!(dhcp::parse_ack(&ack, xid), Some([10, 0, 2, 15]));
        // Rejections.
        assert!(dhcp::parse_ack(&ack, 0x9999_9999).is_none(), "wrong xid");
        let mut not_ack = ack;
        not_ack[242] = 2; // OFFER, not ACK
        assert!(dhcp::parse_ack(&not_ack, xid).is_none(), "not an ack");
    }

    #[test]
    fn icmp_build_then_parse_reply() {
        let src_mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
        let dst_mac = [0x52u8, 0x55, 0x0a, 0x00, 0x02, 0x02];
        let payload = b"kernelOS";
        let mut frame = [0u8; 128];
        let n = icmp::build_echo_request(&src_mac, &dst_mac, [10, 0, 2, 15], [10, 0, 2, 2], 0x1234, 0, payload, &mut frame);
        // IPv4 header self-verifies; the ICMP message checksums to 0.
        assert_eq!(ipv4::checksum(&frame[14..34]), 0);
        assert_eq!(ipv4::checksum(&frame[34..n]), 0, "icmp checksum verifies");
        // As built it is a request, not a reply.
        assert!(!icmp::parse_echo_reply(&frame[..n], 0x1234, 0));
        // Flip the ICMP type to Echo Reply -> parses; rejects wrong ident/seq.
        let mut reply = frame;
        reply[34] = icmp::ICMP_ECHO_REPLY;
        assert!(icmp::parse_echo_reply(&reply[..n], 0x1234, 0));
        assert!(!icmp::parse_echo_reply(&reply[..n], 0x9999, 0), "wrong ident");
        assert!(!icmp::parse_echo_reply(&reply[..n], 0x1234, 7), "wrong seq");
    }

    #[test]
    fn icmp_parse_reply_rejects_non_icmp() {
        let mut frame = [0u8; 64];
        frame[12..14].copy_from_slice(&0x0800u16.to_be_bytes()); // IPv4
        frame[14] = 0x45; // version 4, IHL 5
        frame[14 + 9] = 17; // protocol UDP, not ICMP
        assert!(!icmp::parse_echo_reply(&frame, 0x1234, 0));
    }
}
