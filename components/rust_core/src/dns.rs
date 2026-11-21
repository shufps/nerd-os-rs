//! The captive portal DNS answers (port of `parse_dns_request()` of
//! `components/dns_server/dns_server.c`, the ESP-IDF captive portal
//! example): every A question is answered with the access point's address,
//! so that phones and laptops on the access point open the setup page. The
//! queries come from any client of the open access point. The server
//! task (port of the rest of `dns_server.c`) answers on UDP port 53.
//!
//! Behaviour notes:
//! - the reply holds header, questions and answers only; additional records
//!   of the request (the EDNS0 OPT record that resolvers send by default)
//!   are not copied;
//! - the answer count is the number of answers: a question for another type
//!   (AAAA, ...) gets no answer and is not counted;
//! - every question is parsed and answered in turn;
//! - the opcode is read from the first flags byte; queries with the AD, CD
//!   or Z bit set are answered too (resolvers set AD in queries, RFC 6840);
//! - the reply clears the AD bit of the query (nothing is validated);
//! - a packet that is a response (QR set), shorter than a header or cut off
//!   inside a question gets no reply.
//!
//! Kept: no reply to opcodes other than a standard query, names longer than
//! 128 bytes (as text) and compressed names in questions are refused, the
//! class of the question is echoed, TTL 300 s.

/// The longest name accepted (as dotted text, like the C code's buffer).
pub const MAX_NAME: usize = 128;
/// Replies longer than this are not sent (the C code's buffer).
pub const MAX_REPLY: usize = 256;

const HEADER: usize = 12;
const TYPE_A: u16 = 1;
const TTL: u32 = 300;
const ANSWER_LEN: usize = 16;

/// The reply to `req`, or None for no reply; `ip`: the address bytes in
/// order.
pub fn reply(req: &[u8], ip: [u8; 4]) -> Option<Vec<u8>> {
    if req.len() < HEADER {
        return None;
    }
    let flags = u16::from_be_bytes([req[2], req[3]]);
    // a response, or not a standard query
    if flags & 0x8000 != 0 || flags & 0x7800 != 0 {
        return None;
    }
    let qd_count = u16::from_be_bytes([req[4], req[5]]) as usize;

    // the questions: (offset of the name, type, class), end of the section
    let mut questions = Vec::new();
    let mut pos = HEADER;
    for _ in 0..qd_count {
        let start = pos;
        let mut text_len = 0usize;
        loop {
            let len = *req.get(pos)? as usize;
            if len == 0 && pos > start {
                pos += 1;
                break;
            }
            // as the C code: every label (also an empty first one) counts
            // its length plus a dot; compressed names exceed the limit
            text_len += len + 1;
            if text_len > MAX_NAME {
                return None;
            }
            pos += 1 + len;
            if pos > req.len() {
                return None;
            }
            if len == 0 {
                break;
            }
        }
        let t = req.get(pos..pos + 4)?;
        questions.push((
            start,
            u16::from_be_bytes([t[0], t[1]]),
            u16::from_be_bytes([t[2], t[3]]),
        ));
        pos += 4;
    }

    let answers = questions.iter().filter(|q| q.1 == TYPE_A).count();
    let len = pos + answers * ANSWER_LEN;
    if len > MAX_REPLY {
        return None;
    }
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(&req[..2]);
    // a response; not validated (no AD, RFC 4035 3.2.3)
    out.extend_from_slice(&((flags | 0x8000) & !0x0020).to_be_bytes());
    out.extend_from_slice(&(qd_count as u16).to_be_bytes());
    out.extend_from_slice(&(answers as u16).to_be_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&req[HEADER..pos]);
    for &(name, qtype, class) in questions.iter().filter(|q| q.1 == TYPE_A) {
        out.extend_from_slice(&(0xC000 | name as u16).to_be_bytes());
        out.extend_from_slice(&qtype.to_be_bytes());
        out.extend_from_slice(&class.to_be_bytes());
        out.extend_from_slice(&TTL.to_be_bytes());
        out.extend_from_slice(&4u16.to_be_bytes());
        out.extend_from_slice(&ip);
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// the server (port of dns_server_task() / start_dns_server())

use crate::log::log;

const TAG: &core::ffi::CStr = c"example_dns_redirect_server";
const DNS_PORT: u16 = 53;
/// the C buffer: 128 bytes, one kept for a NUL
const RX_MAX: usize = 127;
/// the reply buffer of the C code
const REPLY_MAX: usize = 256;

#[cfg(target_os = "espidf")]
mod sys {
    pub use crate::idf::raw::{esp_netif_get_handle_from_ifkey, esp_netif_get_ip_info};
}

/// The access point's address (`None` before it has one).
#[cfg(target_os = "espidf")]
fn ap_ip() -> Option<[u8; 4]> {
    let mut info = crate::idf::raw::esp_netif_ip_info_t::default();
    // SAFETY: the if-key is a static NUL-terminated literal; returns null if not found
    let netif = unsafe { sys::esp_netif_get_handle_from_ifkey(c"WIFI_AP_DEF".as_ptr()) };
    // SAFETY: `info` is a live local; esp_netif_get_ip_info rejects a null `netif`
    let ok = unsafe { sys::esp_netif_get_ip_info(netif, &mut info) } == 0;
    // IPADDR_ANY: no answer
    (ok && info.ip.addr != 0).then(|| info.ip.addr.to_ne_bytes())
}

#[cfg(not(target_os = "espidf"))]
fn ap_ip() -> Option<[u8; 4]> {
    None
}

fn serve() {
    use std::net::UdpSocket;
    loop {
        let sock = match UdpSocket::bind(("0.0.0.0", DNS_PORT)) {
            Ok(s) => s,
            Err(e) => {
                log!(Error, TAG, "Socket unable to bind: {e}");
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
        };
        log!(Info, TAG, "Socket created");
        log!(Info, TAG, "Socket bound, port {DNS_PORT}");
        let mut buf = [0u8; RX_MAX];
        loop {
            let (len, from) = match sock.recv_from(&mut buf) {
                Ok(r) => r,
                Err(e) => {
                    log!(Error, TAG, "recvfrom failed: {e}");
                    break;
                }
            };
            let Some(r) = ap_ip().and_then(|ip| reply(&buf[..len], ip)) else {
                continue;
            };
            if r.len() > REPLY_MAX {
                continue;
            }
            if let Err(e) = sock.send_to(&r, from) {
                log!(Error, TAG, "Error occurred during sending: {e}");
                break;
            }
        }
        log!(Error, TAG, "Shutting down socket");
    }
}

unsafe extern "C" fn server_task(_: *mut core::ffi::c_void) {
    serve()
}

/// The captive portal DNS server: every A question answered with the access
/// point's address (a task in internal RAM, as before).
pub fn start_server() {
    crate::idf::task::create_internal(server_task, c"dns_server", 4096, 5);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(
        id: u16,
        flags: u16,
        questions: &[(&str, u16)],
        additional: &[u8],
        ar: u16,
    ) -> Vec<u8> {
        let mut q = Vec::new();
        q.extend_from_slice(&id.to_be_bytes());
        q.extend_from_slice(&flags.to_be_bytes());
        q.extend_from_slice(&(questions.len() as u16).to_be_bytes());
        q.extend_from_slice(&[0, 0, 0, 0]);
        q.extend_from_slice(&ar.to_be_bytes());
        for (name, t) in questions {
            for l in name.split('.').filter(|l| !l.is_empty()) {
                q.push(l.len() as u8);
                q.extend_from_slice(l.as_bytes());
            }
            q.push(0);
            q.extend_from_slice(&t.to_be_bytes());
            q.extend_from_slice(&1u16.to_be_bytes());
        }
        q.extend_from_slice(additional);
        q
    }

    const IP: [u8; 4] = [192, 168, 4, 1];
    const OPT: [u8; 11] = [0, 0, 41, 0x10, 0, 0, 0, 0, 0, 0, 0];

    #[test]
    fn a_query() {
        let q = query(
            0x1234,
            0x0100,
            &[("connectivitycheck.gstatic.com", 1)],
            &[],
            0,
        );
        let r = reply(&q, IP).unwrap();
        assert_eq!(&r[..2], &[0x12, 0x34]);
        assert_eq!(&r[2..4], &[0x81, 0x00]);
        assert_eq!(&r[4..12], &[0, 1, 0, 1, 0, 0, 0, 0]);
        assert_eq!(&r[12..q.len()], &q[12..]);
        assert_eq!(
            &r[q.len()..],
            &[0xC0, 12, 0, 1, 0, 1, 0, 0, 1, 44, 0, 4, 192, 168, 4, 1]
        );
    }

    #[test]
    fn edns_opt_record_is_dropped() {
        let q = query(1, 0x0100, &[("captive.apple.com", 1)], &OPT, 1);
        let r = reply(&q, IP).unwrap();
        // no additional records, the answer right after the question
        assert_eq!(&r[10..12], &[0, 0]);
        let qend = q.len() - OPT.len();
        assert_eq!(r.len(), qend + 16);
        assert_eq!(&r[qend + 12..], &IP);
    }

    #[test]
    fn other_types_unanswered() {
        let q = query(1, 0x0100, &[("example.com", 28)], &[], 0);
        let r = reply(&q, IP).unwrap();
        assert_eq!(&r[6..8], &[0, 0]);
        assert_eq!(r.len(), q.len());
    }

    #[test]
    fn several_questions() {
        let q = query(1, 0x0100, &[("a.b", 1), ("cc.d", 28), ("e.f", 1)], &[], 0);
        let r = reply(&q, IP).unwrap();
        assert_eq!(&r[6..8], &[0, 2]);
        // the answers point at the first and the third name
        let second = 12 + 5 + 4;
        let third = second + 6 + 4;
        assert_eq!(&r[q.len()..q.len() + 2], &[0xC0, 12]);
        assert_eq!(&r[q.len() + 16..q.len() + 18], &[0xC0, third as u8]);
    }

    #[test]
    fn refused() {
        // response, other opcode, short, cut off, compressed, too long
        assert!(reply(&query(1, 0x8100, &[("a.b", 1)], &[], 0), IP).is_none());
        assert!(reply(&query(1, 0x2800, &[("a.b", 1)], &[], 0), IP).is_none());
        assert!(reply(&[0; 11], IP).is_none());
        let q = query(1, 0x0100, &[("a.b", 1)], &[], 0);
        assert!(reply(&q[..q.len() - 1], IP).is_none());
        let mut c = q[..12].to_vec();
        c.extend_from_slice(&[0xC0, 12, 0, 1, 0, 1]);
        assert!(reply(&c, IP).is_none());
        let long = vec!["x".repeat(63); 3].join(".");
        assert!(reply(&query(1, 0x0100, &[(&long, 1)], &[], 0), IP).is_none());
        // the reply would exceed 256 bytes
        let many: Vec<(&str, u16)> = vec![("abcdefgh.ij", 1); 12];
        assert!(reply(&query(1, 0x0100, &many, &[], 0), IP).is_none());
    }

    #[test]
    fn ad_bit_cleared() {
        let r = reply(&query(1, 0x0120, &[("a.b", 1)], &[], 0), IP).unwrap();
        assert_eq!(&r[2..4], &[0x81, 0x00]);
    }

    #[test]
    fn no_questions() {
        let r = reply(&query(7, 0x0100, &[], &[], 0), IP).unwrap();
        assert_eq!(r, vec![0, 7, 0x81, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }
}
