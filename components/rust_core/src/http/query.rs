//! URL parts of HTTP API requests (port of the query handling of
//! `GET /api/system/info`, `GET /api/v2/dashboard`, `POST
//! /api/system/reset-stats` and the node IDs and actions of
//! `/api/v2/can/nodes/*`). The numbers are read like newlib did on the
//! device: `strtoul()` with a 32-bit `unsigned long`, `strtoull()`, `atoi()`
//! with a 32-bit `long`.

/// `httpd_query_key_value()` (ESP-IDF): the value of `key` (compared without
/// case) in `name=value&...`; `None` if missing or if it needs more than
/// `cap` bytes with the NUL (the C callers ignored truncated values).
pub fn query_value<'a>(query: &'a [u8], key: &[u8], cap: usize) -> Option<&'a [u8]> {
    let query = crate::util::cutil::until_nul(query);
    let mut rest = query;
    while !rest.is_empty() {
        let eq = rest.iter().position(|&c| c == b'=')?;
        if eq != key.len() || !rest[..eq].eq_ignore_ascii_case(key) {
            // the next pair starts after the next '&' (searched from the '=')
            let amp = rest[eq..].iter().position(|&c| c == b'&')?;
            rest = &rest[eq + amp + 1..];
            continue;
        }
        let value = &rest[eq + 1..];
        let value = &value[..value.iter().position(|&c| c == b'&').unwrap_or(value.len())];
        return (value.len() < cap).then_some(value);
    }
    None
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// newlib's unsigned `strtoul`/`strtoull` in base 10 for a `max`-sized
/// result: overflow gives `max`, a minus sign negates (modulo the size).
fn strtou(s: &[u8], max: u64) -> u64 {
    let mut i = s.iter().take_while(|&&c| is_space(c)).count();
    let negative = match s.get(i) {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    let (mut acc, mut overflow) = (0u64, false);
    for &c in s[i.min(s.len())..]
        .iter()
        .take_while(|c| c.is_ascii_digit())
    {
        let d = (c - b'0') as u64;
        match acc.checked_mul(10).and_then(|v| v.checked_add(d)) {
            Some(v) if v <= max && !overflow => acc = v,
            _ => overflow = true,
        }
    }
    if overflow {
        max
    } else if negative {
        acc.wrapping_neg() & max
    } else {
        acc
    }
}

pub fn strtoul32(s: &[u8]) -> u32 {
    strtou(s, u32::MAX as u64) as u32
}

pub fn strtoull(s: &[u8]) -> u64 {
    strtou(s, u64::MAX)
}

/// newlib `atoi()`: `strtol()` with a 32-bit `long`, saturating.
pub fn atoi(s: &[u8]) -> i32 {
    let mut i = s.iter().take_while(|&&c| is_space(c)).count();
    let negative = match s.get(i) {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    let mut acc: i64 = 0;
    for &c in s[i.min(s.len())..]
        .iter()
        .take_while(|c| c.is_ascii_digit())
    {
        acc = (acc * 10 + (c - b'0') as i64).min(1 << 32);
    }
    let v = if negative { -acc } else { acc };
    v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

/// The history window a client asks for.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HistoryQuery {
    /// `ts`: start (client time, ms); 0: none
    pub start: u64,
    /// `cur`: the client's current time (ms)
    pub current: u64,
    /// `historySpan` (v2) / `history_span` (v1), ms
    pub span_ms: u64,
    /// `limit`: at most this many samples (0: no limit)
    pub limit: u32,
    /// history requested (`ts` given and not 0)
    pub requested: bool,
    pub has_current: bool,
}

const DEFAULT_SPAN_MS: u64 = 3600 * 1000;
const MAX_SPAN_MS: u64 = 3 * 3600 * 1000;

/// `ts`, `limit`, the span and `cur` from the query (`None`: no query).
/// Values longer than 63 characters are ignored, as before.
pub fn history_query(query: Option<&[u8]>, v2: bool) -> HistoryQuery {
    let mut q = HistoryQuery {
        span_ms: DEFAULT_SPAN_MS,
        ..Default::default()
    };
    let Some(query) = query else {
        return q;
    };
    let get = |key: &str| query_value(query, key.as_bytes(), 64);
    if let Some(v) = get("ts") {
        q.start = strtoull(v);
        q.requested = q.start != 0;
    }
    if let Some(v) = get("limit") {
        q.limit = strtoul32(v).min(1000);
    }
    if let Some(v) = get(if v2 { "historySpan" } else { "history_span" }) {
        q.span_ms = match strtoull(v).min(MAX_SPAN_MS) {
            0 => DEFAULT_SPAN_MS,
            span => span,
        };
    }
    if let Some(v) = get("cur") {
        q.current = strtoull(v);
        q.has_current = true;
    }
    q
}

/// `allTime` is `1` or `true` (at most 7 characters).
pub fn all_time(query: Option<&[u8]>) -> bool {
    query
        .and_then(|q| query_value(q, b"allTime", 8))
        .is_some_and(|v| v == b"1" || v == b"true")
}

pub const CAN_NODE_MAX: i32 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeId {
    Missing,
    Invalid,
    Id(u8),
}

fn node_id(text: &[u8]) -> NodeId {
    match atoi(text) {
        id if (1..CAN_NODE_MAX).contains(&id) => NodeId::Id(id as u8),
        _ => NodeId::Invalid,
    }
}

/// `/api/v2/can/nodes/{id}`: the number after the last `/`.
pub fn can_node_id(uri: &[u8]) -> NodeId {
    let uri = crate::util::cutil::until_nul(uri);
    match uri.iter().rposition(|&c| c == b'/') {
        Some(slash) if slash + 1 < uri.len() => node_id(&uri[slash + 1..]),
        _ => NodeId::Missing,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeAction {
    Restart = 0,
    Shutdown = 1,
    Identify = 2,
}

/// `/api/v2/can/nodes/{id}/{action}`: the ID (`Invalid` if there is none),
/// then the action (`None`: unknown).
pub fn can_node_action(uri: &[u8]) -> (NodeId, Option<NodeAction>) {
    let uri = crate::util::cutil::until_nul(uri);
    let Some(last) = uri.iter().rposition(|&c| c == b'/').filter(|&l| l > 0) else {
        return (NodeId::Invalid, None);
    };
    // the '/' before the last one (or the first character)
    let prev = uri[..last].iter().rposition(|&c| c == b'/').unwrap_or(0);
    if uri[prev] != b'/' {
        return (NodeId::Invalid, None);
    }
    let id = node_id(&uri[prev + 1..]);
    let action = match &uri[last + 1..] {
        b"restart" => Some(NodeAction::Restart),
        b"shutdown" => Some(NodeAction::Shutdown),
        b"identify" => Some(NodeAction::Identify),
        _ => None,
    };
    (id, action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_values() {
        assert_eq!(query_value(b"a=1&TS=5&ts=6", b"ts", 64), Some(&b"5"[..]));
        assert_eq!(query_value(b"tsx=1&ts=", b"ts", 64), Some(&b""[..]));
        assert_eq!(query_value(b"ts", b"ts", 64), None);
        assert_eq!(query_value(b"ts=12345678", b"ts", 8), None); // truncated
        assert_eq!(query_value(b"a=b=c&ts=1", b"ts", 64), Some(&b"1"[..]));
    }

    #[test]
    fn numbers_like_newlib() {
        assert_eq!(strtoul32(b"-1"), u32::MAX);
        assert_eq!(strtoul32(b"4294967296"), u32::MAX);
        assert_eq!(strtoull(b" +12x"), 12);
        assert_eq!(atoi(b"99999999999"), i32::MAX);
        assert_eq!(atoi(b"-99999999999"), i32::MIN);
        assert_eq!(atoi(b"3?x"), 3);
    }

    #[test]
    fn node_routes() {
        assert_eq!(can_node_id(b"/api/v2/can/nodes/3"), NodeId::Id(3));
        assert_eq!(can_node_id(b"/api/v2/can/nodes/"), NodeId::Missing);
        assert_eq!(can_node_id(b"/api/v2/can/nodes/32"), NodeId::Invalid);
        assert_eq!(
            can_node_action(b"/api/v2/can/nodes/5/restart"),
            (NodeId::Id(5), Some(NodeAction::Restart))
        );
        assert_eq!(can_node_action(b"/api/v2/can/nodes/5/x").1, None);
    }
}
