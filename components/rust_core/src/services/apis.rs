//! Responses of the mempool API (port of the parsing in `apis_task.cpp`):
//! Bitcoin price, block height, network hashrate and difficulty, fees. The
//! responses come from the internet (a custom mempool URL may be plain
//! HTTP), read with ArduinoJson's rules ([`crate::util::ajson`]); the HTTP client
//! is `services/apis_fetcher.rs`.
//!
//! Hashrate and difficulty are converted with saturation (negative and NaN:
//! 0, out of range: `u64::MAX`).

use crate::util::ajson;

/// What the fetcher shows.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Values {
    pub price_usd: u32,
    pub block_height: u32,
    /// EH/s
    pub net_hash: u64,
    /// T
    pub net_difficulty: u64,
    pub hour_fee: u32,
    pub half_hour_fee: u32,
    pub fastest_fee: u32,
}

/// The mempool API endpoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    Price = 0,
    BlockHeight = 1,
    Hashrate = 2,
    Fees = 3,
}

/// Parses a response of `api` into `values` (only its fields); false for
/// invalid JSON, which leaves `values` as it was.
pub fn parse(api: Api, body: &[u8], values: &mut Values) -> bool {
    let Ok(doc) = ajson::deserialize(body) else {
        return false;
    };
    match api {
        Api::Price => values.price_usd = doc.get("USD").to_u32(),
        Api::BlockHeight => values.block_height = doc.to_u32(),
        Api::Hashrate => {
            // `as` saturates: negative and NaN give 0
            values.net_hash = (doc.get("currentHashrate").to_f64() / 1e18) as u64;
            values.net_difficulty = (doc.get("currentDifficulty").to_f64() / 1e12) as u64;
        }
        Api::Fees => {
            values.hour_fee = doc.get("hourFee").to_u32();
            values.half_hour_fee = doc.get("halfHourFee").to_u32();
            values.fastest_fee = doc.get("fastestFee").to_u32();
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mempool_responses() {
        let mut v = Values::default();
        assert!(parse(
            Api::Price,
            br#"{"time":1,"USD":97123,"EUR":90000}"#,
            &mut v
        ));
        assert!(parse(Api::BlockHeight, b"873456", &mut v));
        assert!(parse(
            Api::Hashrate,
            br#"{"hashrates":[],"currentHashrate":7.5e20,"currentDifficulty":1.1e14}"#,
            &mut v
        ));
        assert!(parse(
            Api::Fees,
            br#"{"fastestFee":12,"halfHourFee":8,"hourFee":5}"#,
            &mut v
        ));
        assert_eq!(
            v,
            Values {
                price_usd: 97123,
                block_height: 873456,
                // 7.5e20 is read in float32 (7.4999997e20), as before
                net_hash: 749,
                net_difficulty: 110,
                hour_fee: 5,
                half_hour_fee: 8,
                fastest_fee: 12
            }
        );
        // nonsense from a server: saturated values
        assert!(parse(
            Api::Hashrate,
            br#"{"currentHashrate":-1e30,"currentDifficulty":1e300}"#,
            &mut v
        ));
        assert_eq!((v.net_hash, v.net_difficulty), (0, u64::MAX));
        assert!(!parse(Api::Price, b"<html>", &mut v));
        assert_eq!(v.price_usd, 97123);
    }
}
