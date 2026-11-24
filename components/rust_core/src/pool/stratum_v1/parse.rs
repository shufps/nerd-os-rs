//! Parsing of pool messages (port of `StratumApi::parse`).

use super::session::Request;
use super::*;
use crate::util::ajson::{self, Json};
use crate::util::cutil::{hex2bin, strtoul32_hex, until_nul};

/// Whether `line` is one JSON value as ArduinoJson reads it (the former
/// `deserializeJson()` check of the connection loop).
pub fn is_json(line: &[u8]) -> bool {
    ajson::deserialize(line).is_ok()
}

/// Parses one JSON-RPC line into `message`, classifying responses by the
/// former fixed IDs (see [`Session`](super::session::Session) for matching by
/// request). Like the C++ version, `method` is set before validation, so it
/// is meaningful even on failure (the caller uses it to detect a bad
/// subscribe response).
pub fn parse(line: &[u8], message: &mut Message) -> bool {
    parse_with(line, message, &mut Request::legacy)
}

/// [`parse`], with `classify` telling which request a response ID answers.
pub fn parse_with(
    line: &[u8],
    message: &mut Message,
    classify: &mut dyn FnMut(i64) -> Option<Request>,
) -> bool {
    let Ok(doc) = ajson::deserialize(line) else {
        return false;
    };

    message.message_id = doc.get("id").as_int().map_or(-1, i64::from);

    match doc.get("method").as_cstr() {
        Some(method_str) => parse_method(&doc, method_str, message),
        None => parse_response(&doc, classify(message.message_id), message),
    }
}

fn parse_method(doc: &Json, method_str: &[u8], message: &mut Message) -> bool {
    message.method = match method_str {
        b"mining.notify" => method::MINING_NOTIFY,
        b"mining.set_difficulty" => method::MINING_SET_DIFFICULTY,
        b"mining.set_version_mask" => method::MINING_SET_VERSION_MASK,
        b"client.reconnect" => method::CLIENT_RECONNECT,
        b"mining.set_extranonce" => method::MINING_SET_EXTRANONCE,
        _ => {
            message.method = method::STRATUM_UNKNOWN;
            return false;
        }
    };

    let params = doc.get("params");
    match message.method {
        method::MINING_NOTIFY => parse_notify(params, message),
        method::MINING_SET_DIFFICULTY => {
            message.new_difficulty = params.at(0).to_u32();
            true
        }
        method::MINING_SET_VERSION_MASK => match params.at(0).as_cstr() {
            Some(mask) => {
                message.version_mask = strtoul32_hex(mask);
                true
            }
            None => false,
        },
        method::MINING_SET_EXTRANONCE => {
            // {"id": null, "method": "mining.set_extranonce", "params": ["<extranonce1>", <extranonce2_size>]}
            params.len() >= 2 && set_extranonce(params.at(0), params.at(1), message)
        }
        _ => true,
    }
}

/// `mining.notify` params: job id, prevhash, coinbase 1, coinbase 2, merkle
/// branches, version, nbits, ntime, clean jobs.
fn parse_notify(params: &Json, message: &mut Message) -> bool {
    let field = |i| params.at(i).as_cstr();
    let (Some(job_id), Some(prev_hash), Some(coinbase_1), Some(coinbase_2)) =
        (field(0), field(1), field(2), field(3))
    else {
        return false;
    };
    let merkle = params.at(4);
    let n_merkle = merkle.len();
    if n_merkle > MAX_MERKLE_BRANCHES {
        return false;
    }
    let Some(branches) = (0..n_merkle)
        .map(|i| merkle.at(i).as_cstr())
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    let (Some(version), Some(nbits), Some(ntime)) = (field(5), field(6), field(7)) else {
        return false;
    };

    let hash = |hex| {
        let mut h = [0; HASH_SIZE];
        hex2bin(hex, &mut h);
        h
    };
    message.mining_notification = Some(MiningNotify {
        job_id: until_nul(job_id).to_vec(),
        prev_block_hash: hash(prev_hash),
        coinbase_1: until_nul(coinbase_1).to_vec(),
        coinbase_2: until_nul(coinbase_2).to_vec(),
        merkle_branches: branches.into_iter().map(hash).collect(),
        version: strtoul32_hex(version),
        target: strtoul32_hex(nbits),
        ntime: strtoul32_hex(ntime),
    });
    message.should_abandon_work = params.at(params.len() - 1).truthy();
    true
}

fn set_extranonce(extranonce: &Json, extranonce2_len: &Json, message: &mut Message) -> bool {
    message.extranonce_2_len = extranonce2_len.to_i32();
    let Some(extranonce) = extranonce.as_cstr() else {
        return false;
    };
    if !EXTRANONCE2_LEN.contains(&message.extranonce_2_len) {
        return false;
    }
    message.extranonce_str = Some(until_nul(extranonce).to_vec());
    true
}

/// A response to `request` (None: not a known request).
fn parse_response(doc: &Json, request: Option<Request>, message: &mut Message) -> bool {
    let result = doc.get("result");
    match request {
        Some(Request::Subscribe) => {
            message.method = method::STRATUM_RESULT_SUBSCRIBE;
            result.len() >= 3 && set_extranonce(result.at(1), result.at(2), message)
        }
        Some(Request::Configure) => {
            message.method = method::STRATUM_RESULT_VERSION_MASK;
            match result.get("version-rolling.mask").as_cstr() {
                Some(mask) => {
                    message.version_mask = strtoul32_hex(mask);
                    true
                }
                None => false,
            }
        }
        Some(Request::Authorize | Request::SuggestDifficulty | Request::ExtranonceSubscribe) => {
            message.method = method::STRATUM_RESULT_SETUP;
            message.response_success = parse_result(doc);
            true
        }
        Some(Request::Submit) => {
            message.method = method::STRATUM_RESULT;
            message.response_success = parse_result(doc);
            true
        }
        None => {
            message.method = method::STRATUM_UNKNOWN;
            false
        }
    }
}

/// true only for `"result": true` without an error.
fn parse_result(doc: &Json) -> bool {
    doc.get("error").is_null() && doc.get("result").as_bool().unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Parsed(Message, bool);

    fn parse_str(s: &str) -> Parsed {
        let mut m = Message::default();
        let ok = parse(s.as_bytes(), &mut m);
        Parsed(m, ok)
    }

    const NOTIFY: &str = r#"{"params":["67a1","4d16b6f85af6e2198f44ae2a6de67f78487ae5611b77c6c0440b921e00000000","01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff20020862062f503253482f04b8864e5008","072f736c7573682f000000000100f2052a010000001976a914d23fcdf86f7e756a64a7a9688ef9903327048ed988ac00000000",["cd0b41a3b1a17d6e8bb9ce9a3ff0a1c2e2b3e5b1f1a6c3d9e8f7a6b5c4d3e2f1"],"20000000","1703a30c","66b9a2f0",true],"id":null,"method":"mining.notify"}"#;

    #[test]
    fn mining_notify() {
        let p = parse_str(NOTIFY);
        assert!(p.1);
        assert_eq!(
            (p.0.method, p.0.message_id, p.0.should_abandon_work),
            (method::MINING_NOTIFY, -1, true)
        );
        let n = p.0.mining_notification.as_ref().unwrap();
        assert_eq!(n.job_id, b"67a1");
        assert_eq!(&n.prev_block_hash[..4], &[0x4d, 0x16, 0xb6, 0xf8]);
        assert_eq!(
            (n.merkle_branches.len(), n.merkle_branches[0][0]),
            (1, 0xcd)
        );
        assert_eq!(
            (n.version, n.target, n.ntime),
            (0x2000_0000, 0x1703_a30c, 0x66b9_a2f0)
        );
    }

    #[test]
    fn notify_with_missing_fields_is_rejected() {
        for bad in [
            r#"{"id":null,"method":"mining.notify","params":[]}"#,
            r#"{"id":null,"method":"mining.notify","params":{"a":1}}"#,
            r#"{"id":null,"method":"mining.notify","params":["1","00","aa","bb",[],"20000000",1,"0",false]}"#,
            r#"{"id":null,"method":"mining.notify","params":["1","00","aa","bb",[7],"2","1","0",false]}"#,
        ] {
            let p = parse_str(bad);
            assert!(!p.1, "{bad}");
            assert_eq!(p.0.method, method::MINING_NOTIFY);
        }
    }

    #[test]
    fn subscribe_response() {
        let p = parse_str(
            r#"{"id":1,"result":[[["mining.notify","ae6812eb4cd7735a302a8a9dd95cf71f"]],"08000002",4],"error":null}"#,
        );
        assert!(p.1);
        assert_eq!(
            (p.0.method, p.0.extranonce_2_len),
            (method::STRATUM_RESULT_SUBSCRIBE, 4)
        );
        assert_eq!(p.0.extranonce_str.as_deref(), Some(&b"08000002"[..]));

        for en2 in ["0", "-1", "17", "-2147483648"] {
            let p = parse_str(&format!(
                r#"{{"id":1,"result":[[],"08000002",{en2}],"error":null}}"#
            ));
            assert!(!p.1, "extranonce2_size {en2}");
            assert_eq!(p.0.method, method::STRATUM_RESULT_SUBSCRIBE);
        }
    }

    #[test]
    fn results_and_simple_methods() {
        let cases: &[(&str, bool, i32)] = &[
            (
                r#"{"id":2,"result":{"version-rolling":true,"version-rolling.mask":"1fffe000"},"error":null}"#,
                true,
                method::STRATUM_RESULT_VERSION_MASK,
            ),
            (
                r#"{"id":3,"result":true,"error":null}"#,
                true,
                method::STRATUM_RESULT_SETUP,
            ),
            (
                r#"{"id":7,"result":true,"error":null}"#,
                true,
                method::STRATUM_RESULT,
            ),
            (
                r#"{"id":7,"result":null,"error":[23,"Low difficulty share",null]}"#,
                true,
                method::STRATUM_RESULT,
            ),
            (
                r#"{"id":null,"method":"mining.set_difficulty","params":[0.5]}"#,
                true,
                method::MINING_SET_DIFFICULTY,
            ),
            (
                r#"{"id":null,"method":"mining.set_version_mask","params":[536862720]}"#,
                false,
                method::MINING_SET_VERSION_MASK,
            ),
            (
                r#"{"id":null,"method":"client.reconnect","params":[]}"#,
                true,
                method::CLIENT_RECONNECT,
            ),
            (
                r#"{"id":null,"method":"mining.ping","params":[]}"#,
                false,
                method::STRATUM_UNKNOWN,
            ),
            (r#"{"id":0,"result":true}"#, false, method::STRATUM_UNKNOWN),
        ];
        for &(json, ok, method) in cases {
            let p = parse_str(json);
            assert_eq!((p.1, p.0.method), (ok, method), "{json}");
        }
        assert_eq!(parse_str(cases[0].0).0.version_mask, 0x1fff_e000);
        assert!(parse_str(cases[2].0).0.response_success);
        assert!(!parse_str(cases[3].0).0.response_success);
        assert_eq!(parse_str(cases[4].0).0.new_difficulty, 0); // truncated, like ArduinoJson
    }

    #[test]
    fn json_syntax_as_arduinojson_reads_it() {
        // what ArduinoJson accepted before step 1, accepted again
        for ok in [
            &br#"{"id":1}"#[..],
            b"{'id':1}",
            b"{id:1}",
            br#"{"id":1} x"#,
        ] {
            assert!(is_json(ok), "{}", String::from_utf8_lossy(ok));
        }
        for bad in [&b""[..], b"{", b"{\"id\":}", b"12 "] {
            assert!(!is_json(bad));
            assert!(!parse(bad, &mut Message::default()));
        }
    }
}
