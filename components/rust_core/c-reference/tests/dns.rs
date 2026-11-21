//! Differential tests of the captive portal DNS answers: rust_core::dns
//! against the former parse_dns_request() (c-dns/, the ESP-IDF example,
//! verbatim) with the firmware's rule ("*" -> the access point address):
//! single-question queries give the same bytes; the intended differences
//! (no answer counted for other types, EDNS records left out) are checked
//! as such; random packets never panic and only give well-formed replies.

use c_reference as _;
use rust_core::dns;

extern "C" {
    fn ref_dns_reply(
        req: *const u8,
        len: usize,
        ip: *const u8,
        out: *mut u8,
        out_len: usize,
    ) -> usize;
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn reference(req: &[u8], ip: [u8; 4]) -> Option<Vec<u8>> {
    // the reference keeps the address and its rule table in statics
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = [0u8; 256];
    let n = unsafe {
        ref_dns_reply(
            req.as_ptr(),
            req.len(),
            ip.as_ptr(),
            out.as_mut_ptr(),
            out.len(),
        )
    };
    (n > 0).then(|| out[..n].to_vec())
}

fn query(rng: &mut Rng, qtype: u16, opt: bool) -> (Vec<u8>, usize) {
    let mut q = Vec::new();
    q.extend_from_slice(&(rng.next() as u16).to_be_bytes());
    let flags = [0x0100u16, 0x0000, 0x0120, 0x0110][rng.below(4) as usize];
    q.extend_from_slice(&flags.to_be_bytes());
    q.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, opt as u8]);
    for _ in 0..1 + rng.below(3) {
        let n = 1 + rng.below(15) as usize;
        q.push(n as u8);
        for _ in 0..n {
            q.push(b"abcdefghijklmnopqrstuvwxyz0123456789-"[rng.below(37) as usize]);
        }
    }
    q.push(0);
    q.extend_from_slice(&qtype.to_be_bytes());
    q.extend_from_slice(&[0, 1]);
    let qend = q.len();
    if opt {
        q.extend_from_slice(&[0, 0, 41, 0x10, 0, 0, 0, 0, 0, 0, 0]);
    }
    (q, qend)
}

#[test]
fn single_questions_match_the_former_code() {
    let mut rng = Rng(0xd15_c0de_0000_0001);
    let ip = [192, 168, 4, 1];
    for _ in 0..20_000 {
        let qtype = [1u16, 1, 28, 15, 255, 65][rng.below(6) as usize];
        let (q, _) = query(&mut rng, qtype, false);
        let new = dns::reply(&q, ip).unwrap();
        let Some(old) = reference(&q, ip) else {
            // intended difference: the reference drops queries with the
            // AD, CD or Z bit set; the Rust version answers them
            assert_ne!(q[3] & 0x78, 0, "{q:02x?}");
            continue;
        };
        if qtype == 1 {
            assert_eq!(new, old, "{q:02x?}");
        } else {
            // intended difference: the reference counts an answer and
            // appends 16 zero bytes; the Rust version sends no answer
            assert_eq!(&old[6..8], &[0, 1]);
            assert_eq!(&new[6..8], &[0, 0]);
            assert_eq!(old.len(), new.len() + 16);
            assert!(old[new.len()..].iter().all(|&b| b == 0));
            assert_eq!(
                (&new[..6], &new[8..]),
                (&old[..6], &old[8..new.len()]),
                "{q:02x?}"
            );
        }
    }
}

#[test]
fn edns_queries_get_a_readable_answer() {
    let mut rng = Rng(0xd15_c0de_0000_0002);
    let ip = [10, 0, 0, 1];
    for _ in 0..5_000 {
        let (q, qend) = query(&mut rng, 1, true);
        let Some(old) = reference(&q, ip) else {
            assert_ne!(q[3] & 0x78, 0);
            continue;
        };
        let new = dns::reply(&q, ip).unwrap();
        // the former reply: the OPT record between question and answer,
        // still counted as additional record
        assert_eq!(&old[10..12], &[0, 1]);
        assert_eq!(&old[qend..q.len()], &q[qend..]);
        // now: the same header without it, the answer after the question
        assert_eq!(&new[10..12], &[0, 0]);
        assert_eq!(&new[..10], &old[..10]);
        assert_eq!(&new[12..qend], &old[12..qend]);
        assert_eq!(&new[qend..], &old[q.len()..]);
    }
}

#[test]
fn random_packets() {
    let mut rng = Rng(0xd15_c0de_0000_0003);
    for _ in 0..200_000 {
        let n = rng.below(128) as usize;
        let mut p: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        if n >= 6 && rng.below(2) == 0 {
            // mostly standard queries with few questions
            p[2] &= 0x07;
            p[4] = 0;
            p[5] = rng.below(4) as u8;
        }
        if let Some(r) = dns::reply(&p, [1, 2, 3, 4]) {
            assert!(r.len() >= 12 && r.len() <= dns::MAX_REPLY);
            assert_eq!(&r[..2], &p[..2]);
            assert_eq!(r[2] & 0x80, 0x80);
            let an = u16::from_be_bytes([r[6], r[7]]) as usize;
            assert_eq!(&r[8..12], &[0, 0, 0, 0]);
            // header, the questions as sent, 16 bytes per answer
            let qlen = r.len() - 12 - an * 16;
            assert_eq!(&r[12..12 + qlen], &p[12..12 + qlen]);
        }
    }
}
