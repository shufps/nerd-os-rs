//! The configuration (port of the in-memory document of
//! `main/nvs_config.cpp`): a flat JSON object of setting name to value,
//! loaded from and saved to one NVS blob, with the typed getters and
//! setters the firmware uses. Values convert like ArduinoJson's `as<T>()`,
//! and the document is written byte for byte as `serializeJson()` did, so
//! older firmware reads it back. The NVS access and the migration from
//! the old per-setting keys stay in C++.

use core::ffi::CStr;
use std::sync::Mutex;

use crate::ajson::{self, Json, Num};
use crate::log::log;

const TAG: &CStr = c"nvs_config";

struct Store {
    /// members in order; setting an existing key replaces it in place
    doc: Vec<(Vec<u8>, Json)>,
    dirty: bool,
}

static STORE: Mutex<Store> = Mutex::new(Store {
    doc: Vec::new(),
    dirty: false,
});

fn with<T>(f: impl FnOnce(&mut Store) -> T) -> T {
    f(&mut STORE.lock().unwrap_or_else(|e| e.into_inner()))
}

impl Store {
    fn get(&self, key: &[u8]) -> &Json {
        static NULL: Json = Json::Null;
        self.doc
            .iter()
            .find(|(k, _)| k == key)
            .map_or(&NULL, |(_, v)| v)
    }

    fn set(&mut self, key: &[u8], value: Json) {
        match self.doc.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value,
            None => self.doc.push((key.to_vec(), value)),
        }
        self.dirty = true;
    }

    fn serialize(&self) -> Vec<u8> {
        Json::Obj(self.doc.clone()).serialize()
    }
}

/// Replaces the document with the stored JSON; false if it does not parse
/// (then the document is empty). A stored value that is not an object
/// gives an empty document.
pub fn load(json: &[u8]) -> bool {
    match ajson::deserialize(json) {
        Ok(Json::Obj(members)) => {
            with(|s| s.doc = members);
            true
        }
        Ok(_) => {
            with(|s| s.doc.clear());
            true
        }
        Err(_) => {
            with(|s| s.doc.clear());
            false
        }
    }
}

pub fn get_str(key: &[u8]) -> Option<Vec<u8>> {
    with(|s| s.get(key).as_cstr().map(<[u8]>::to_vec))
}

pub fn set(key: &[u8], value: Json) {
    with(|s| s.set(key, value));
}

pub fn get_u16(key: &[u8], def: u16) -> u16 {
    with(|s| match s.get(key) {
        Json::Null => def,
        v => v.to_u16(),
    })
}

pub fn get_u64(key: &[u8], def: u64) -> u64 {
    with(|s| match s.get(key) {
        Json::Null => def,
        v => v.to_u64(),
    })
}

/// `cfgGetBool()`: a bool as it is, other values by `as<uint16_t>() != 0`.
pub fn get_bool(key: &[u8], def: bool) -> bool {
    with(|s| match s.get(key) {
        Json::Null => def,
        Json::Bool(b) => *b,
        v => v.to_u16() != 0,
    })
}

pub fn has(key: &[u8]) -> bool {
    with(|s| !s.get(key).is_null())
}

/// The fixups after a migration from the old per-setting NVS keys (the
/// end of `migrate_from_legacy()`); the key names are those of
/// `nvs_config.h`.
pub fn migration_fixups(single_fan: bool, overheat_default: u16) {
    with(|s| {
        if s.get(b"overheat_temp").as_uint(65535) == Some(0) {
            s.set(b"overheat_temp", Json::Num(Num::U(70)));
            log!(Warn, TAG, "Overheat temp 0 not allowed, set to 70");
        }
        if s.get(b"autofanspeed").as_uint(65535) == Some(1) {
            s.set(b"autofanspeed", Json::Num(Num::U(0)));
            s.set(b"fanspeed", Json::Num(Num::U(100)));
            log!(Warn, TAG, "AFC deprecated, set manual 100%");
        }
        // VReg overheat temp: always off on single-fan boards (pidUseMax
        // covers the VReg); dual-fan boards inherit the ASIC value
        if single_fan {
            s.set(b"fan1_overheat", Json::Num(Num::U(0)));
        } else if s.get(b"fan1_overheat").is_null() {
            let v = match s.get(b"overheat_temp") {
                Json::Null => overheat_default,
                v => v.to_u16(),
            };
            log!(
                Info,
                TAG,
                "Migrating VReg overheat temp from ASIC value: {v}°C"
            );
            s.set(b"fan1_overheat", Json::Num(Num::U(v as u64)));
        }
    });
}

/// If the document changed since the last write: serializes it and passes
/// it to `write` (with the document locked, so no change is lost); a
/// successful write clears the changed flag. False if a write failed.
pub fn flush_with(write: impl FnOnce(&[u8]) -> bool) -> bool {
    with(|s| {
        if !s.dirty {
            return true;
        }
        let json = s.serialize();
        if write(&json) {
            s.dirty = false;
            true
        } else {
            false
        }
    })
}

/// Empties the document.
pub fn clear() {
    with(|s| s.doc.clear());
}

pub fn mark_dirty() {
    with(|s| s.dirty = true);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The store is global.
    pub(crate) static LOCK: Mutex<()> = Mutex::new(());

    fn saved() -> String {
        let mut out = String::new();
        flush_with(|j| {
            out = String::from_utf8(j.to_vec()).unwrap();
            true
        });
        out
    }

    #[test]
    fn set_replaces_in_place_and_serializes() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        assert!(load(b"{\"stratumurl\":\"a\",\"fanspeed\":50}"));
        set(b"newkey", Json::Bool(true));
        set(b"stratumurl", Json::Str(b"b\"c".to_vec()));
        set(b"bestdiff", Json::Num(Num::U(u64::MAX)));
        assert_eq!(
            saved(),
            "{\"stratumurl\":\"b\\\"c\",\"fanspeed\":50,\"newkey\":true,\"bestdiff\":18446744073709551615}"
        );
        // nothing changed: no write
        let mut called = false;
        flush_with(|_| {
            called = true;
            true
        });
        assert!(!called);
        // a failed write keeps it dirty
        mark_dirty();
        assert!(!flush_with(|_| false));
        assert!(flush_with(|_| true));
    }

    #[test]
    fn getters_convert_like_arduinojson() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        assert!(load(
            b"{\"a\":70000,\"b\":\"42\",\"c\":1.9,\"d\":true,\"e\":-1,\"f\":\"x\",\"g\":null,\"h\":0}"
        ));
        assert_eq!(get_u16(b"a", 7), 0); // out of range
        assert_eq!(get_u16(b"b", 7), 42); // number string
        assert_eq!(get_u16(b"c", 7), 1); // truncated
        assert_eq!(get_u16(b"d", 7), 1);
        assert_eq!(get_u16(b"e", 7), 0);
        assert_eq!(get_u16(b"f", 7), 0);
        assert_eq!(get_u16(b"g", 7), 7); // null: default
        assert_eq!(get_u16(b"missing", 7), 7);
        assert_eq!(get_u64(b"a", 0), 70000);
        assert!(get_bool(b"d", false));
        assert!(!get_bool(b"h", true));
        // 70000 as uint16_t is 0: false, as in cfgGetBool()
        assert!(!get_bool(b"a", true));
        assert_eq!(get_str(b"f"), Some(b"x".to_vec()));
        assert_eq!(get_str(b"a"), None);
        assert!(has(b"a") && !has(b"g") && !has(b"missing"));
        // an invalid document: empty, reported
        assert!(!load(b"{oops"));
        assert!(!has(b"a"));
    }

    #[test]
    fn fixups() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        assert!(load(b"{\"overheat_temp\":0,\"autofanspeed\":1}"));
        migration_fixups(false, 70);
        assert_eq!(get_u16(b"overheat_temp", 0), 70);
        assert_eq!(get_u16(b"autofanspeed", 9), 0);
        assert_eq!(get_u16(b"fanspeed", 0), 100);
        assert_eq!(get_u16(b"fan1_overheat", 0), 70);
        assert!(load(b"{}"));
        migration_fixups(false, 75);
        assert_eq!(get_u16(b"fan1_overheat", 0), 75);
        migration_fixups(true, 75);
        assert_eq!(get_u16(b"fan1_overheat", 9), 0);
    }
}
