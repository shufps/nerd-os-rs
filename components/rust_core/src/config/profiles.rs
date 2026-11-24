//! Settings profiles: up to [`MAX_PROFILES`] named sets of ASIC and fan
//! settings that a home automation switches between (e.g. overclocked while
//! the PV system has a surplus, quieter at night). A profile holds a subset
//! of the `PATCH /api/v2/settings` body (see [`KEYS`]); applying it goes the
//! same way as that PATCH and takes effect without a restart. Pool settings
//! are not part of a profile.
//!
//! The profiles are one JSON array in their own NVS blob; applying one only
//! writes the normal config (and the index of the profile last applied).
//! When there has never been a profile (no blob yet: a new device or the
//! first start of this firmware), a profile "default" with the current
//! settings is created and marked active; a list emptied by the user stays
//! empty.
//!
//! Applying needs no OTP: it only selects one of the stored profiles.
//! Changing the profiles needs OTP like any other setting.

use core::ffi::CStr;
use std::sync::Mutex;

use crate::ajson::{self, Json, Num};
use crate::config;
use crate::log::log;

const TAG: &CStr = c"profiles";

pub const MAX_PROFILES: usize = 5;
pub const MAX_NAME: usize = 32;
/// Upper bound of the stored JSON (NVS space).
pub const MAX_BLOB: usize = 2048;

/// The settings a profile may hold (`PATCH /api/v2/settings` names).
pub const KEYS: [&str; 3] = ["frequency", "coreVoltage", "fans"];
/// The members of a fan channel, and of its `pid` object.
const FAN_KEYS: [&str; 3] = ["mode", "manualSpeed", "overheatTemp"];
const PID_KEYS: [&str; 4] = ["targetTemp", "p", "i", "d"];

/// The config key of the profile last applied (none: absent or [`NONE`]).
const ACTIVE_KEY: &[u8] = b"profile_act";
const NONE: u16 = 0xffff;

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: Vec<u8>,
    /// an object with members of [`KEYS`] only
    pub settings: Json,
}

/// Why a profile list was refused (the 400 message).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidJson,
    NotAList,
    TooMany,
    BadName,
    BadSettings,
    TooLarge,
}

impl Error {
    pub fn message(self) -> &'static CStr {
        match self {
            Error::InvalidJson => c"Invalid JSON",
            Error::NotAList => c"Expected {\"profiles\": [...]}",
            Error::TooMany => c"Too many profiles (at most 5)",
            Error::BadName => c"Each profile needs a name (1 to 32 characters)",
            Error::BadSettings => c"Invalid profile settings",
            Error::TooLarge => c"Profiles too large",
        }
    }
}

fn members(v: &Json) -> &[(Vec<u8>, Json)] {
    match v {
        Json::Obj(m) => m,
        _ => &[],
    }
}

/// The members of `v` named in `keys` (in the order received).
fn pick(v: &Json, keys: &[&str]) -> Vec<(Vec<u8>, Json)> {
    members(v)
        .iter()
        .filter(|(k, _)| keys.iter().any(|key| key.as_bytes() == k.as_slice()))
        .cloned()
        .collect()
}

fn is_uint(v: &Json, min: u64, max: u64) -> bool {
    v.as_uint(max).is_some_and(|n| n >= min)
}

fn is_number(v: &Json) -> bool {
    matches!(v, Json::Num(_))
}

/// A fan channel restricted to its known members, or None if invalid.
fn fan(v: &Json) -> Option<Json> {
    if !v.is_object() {
        return None;
    }
    let mut out = pick(v, &FAN_KEYS);
    let ok = out.iter().all(|(k, v)| match k.as_slice() {
        // 0 manual, 1 automatic, 2 PID, 3 (second fan) as the first fan
        b"mode" => is_uint(v, 0, 3),
        b"manualSpeed" => is_uint(v, 0, 100),
        _ => is_uint(v, 1, 0xffff),
    });
    if !ok {
        return None;
    }
    let pid = v.get("pid");
    if !pid.is_null() {
        if !pid.is_object() {
            return None;
        }
        let p = pick(pid, &PID_KEYS);
        // targetTemp is -1 on boards without PID control (not applied then)
        let ok = p.iter().all(|(_, v)| is_number(v));
        if !ok {
            return None;
        }
        out.push((b"pid".to_vec(), Json::Obj(p)));
    }
    Some(Json::Obj(out))
}

/// The settings of a profile restricted to [`KEYS`], or None if a value is
/// invalid. Unknown members are dropped.
pub fn settings(v: &Json) -> Option<Json> {
    if !v.is_object() {
        return None;
    }
    let mut out = Vec::new();
    for (k, v) in pick(v, &KEYS) {
        let v = match k.as_slice() {
            b"frequency" | b"coreVoltage" => is_uint(&v, 1, 0xffff).then_some(v)?,
            _ => {
                let fans = v.as_array()?;
                if fans.len() > 2 {
                    return None;
                }
                Json::Arr(fans.iter().map(fan).collect::<Option<_>>()?)
            }
        };
        out.push((k, v));
    }
    Some(Json::Obj(out))
}

fn profile(v: &Json) -> Result<Profile, Error> {
    let name = v.get("name").as_cstr().ok_or(Error::BadName)?;
    // characters, as the UI's maxlength and the error text (bytes if the
    // name is not UTF-8)
    let len = std::str::from_utf8(name).map_or(name.len(), |s| s.chars().count());
    if name.is_empty() || len > MAX_NAME || name.contains(&0) {
        return Err(Error::BadName);
    }
    let settings = settings(v.get("settings")).ok_or(Error::BadSettings)?;
    Ok(Profile {
        name: name.to_vec(),
        settings,
    })
}

/// The stored form: an array of `{"name", "settings"}`.
pub fn serialize(list: &[Profile]) -> Vec<u8> {
    Json::Arr(
        list.iter()
            .map(|p| {
                Json::Obj(vec![
                    (b"name".to_vec(), Json::Str(p.name.clone())),
                    (b"settings".to_vec(), p.settings.clone()),
                ])
            })
            .collect(),
    )
    .serialize()
}

/// The profiles of a `PATCH /api/v2/profiles` body (`{"profiles": [...]}`).
pub fn parse_patch(body: &[u8]) -> Result<Vec<Profile>, Error> {
    let doc = ajson::deserialize(body).map_err(|_| Error::InvalidJson)?;
    let items = doc.get("profiles").as_array().ok_or(Error::NotAList)?;
    if items.len() > MAX_PROFILES {
        return Err(Error::TooMany);
    }
    let list = items.iter().map(profile).collect::<Result<Vec<_>, _>>()?;
    if serialize(&list).len() > MAX_BLOB {
        return Err(Error::TooLarge);
    }
    Ok(list)
}

/// The stored form back (invalid entries are skipped).
pub fn parse_stored(data: &[u8]) -> Vec<Profile> {
    let Ok(doc) = ajson::deserialize(data) else {
        return Vec::new();
    };
    doc.as_array()
        .unwrap_or_default()
        .iter()
        .filter_map(|p| profile(p).ok())
        .take(MAX_PROFILES)
        .collect()
}

/// The profile of a `POST /api/v2/profiles/apply` body (`{"id": n}`).
pub fn parse_apply(body: &[u8], count: usize) -> Option<usize> {
    let doc = ajson::deserialize(body).ok()?;
    let id = doc.get("id").as_uint(MAX_PROFILES as u64)? as usize;
    (id < count).then_some(id)
}

/// `GET /api/v2/profiles`.
pub fn to_json(list: &[Profile], active: Option<usize>) -> Json {
    let profiles = list
        .iter()
        .enumerate()
        .map(|(i, p)| {
            Json::Obj(vec![
                (b"id".to_vec(), Json::Num(Num::U(i as u64))),
                (b"name".to_vec(), Json::Str(p.name.clone())),
                (b"settings".to_vec(), p.settings.clone()),
            ])
        })
        .collect();
    Json::Obj(vec![
        (b"max".to_vec(), Json::Num(Num::U(MAX_PROFILES as u64))),
        (
            b"active".to_vec(),
            active.map_or(Json::Null, |i| Json::Num(Num::U(i as u64))),
        ),
        (b"profiles".to_vec(), Json::Arr(profiles)),
    ])
}

/// Where the profile last applied (`old[active]`) is in `new`: the same
/// name and settings; None if it was deleted or changed.
pub fn moved(old: &[Profile], active: Option<usize>, new: &[Profile]) -> Option<usize> {
    let p = old.get(active?)?;
    new.iter().position(|n| n == p)
}

/// The index of the profile last applied, if it still exists.
pub fn active(count: usize) -> Option<usize> {
    if !config::has(ACTIVE_KEY) {
        return None;
    }
    let i = config::get_u16(ACTIVE_KEY, 0) as usize;
    (i < count).then_some(i)
}

// ---------------------------------------------------------------------------
// storage

/// The loaded profiles (None: not read from NVS yet).
static PROFILES: Mutex<Option<Vec<Profile>>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut Vec<Profile>) -> T) -> T {
    let mut p = PROFILES.lock().unwrap_or_else(|e| e.into_inner());
    let list = p.get_or_insert_with(|| nvs::read().map_or_else(Vec::new, |d| parse_stored(&d)));
    f(list)
}

/// The profile settings of a `GET /api/v2/settings` document: frequency,
/// core voltage, the fans.
pub fn from_settings(doc: &Json) -> Option<Json> {
    settings(doc)
}

/// Creates the profile "default" from `current()` (the settings document)
/// if there has never been a profile; loads the profiles.
pub fn ensure_default(current: impl FnOnce() -> Json) {
    let mut p = PROFILES.lock().unwrap_or_else(|e| e.into_inner());
    if p.is_some() {
        return;
    }
    let list = match nvs::read() {
        Some(d) => parse_stored(&d),
        None => match from_settings(&current()) {
            Some(settings) => {
                let list = vec![Profile {
                    name: b"default".to_vec(),
                    settings,
                }];
                if nvs::write(&serialize(&list)) {
                    log!(Info, TAG, "profile \"default\" created");
                    config::set(ACTIVE_KEY, Json::Num(Num::U(0)));
                    crate::config::nvs::request_flush();
                }
                list
            }
            None => Vec::new(),
        },
    };
    *p = Some(list);
}

#[cfg(target_os = "espidf")]
mod nvs {
    use super::*;
    use crate::idf::nvs::{err_name, Nvs, READONLY, READWRITE};
    use crate::idf::{EspErr, ESP_OK};

    const NAMESPACE: &CStr = c"main";
    const KEY: &CStr = c"profiles";

    /// None: there is no stored list (never saved).
    pub fn read() -> Option<Vec<u8>> {
        match Nvs::open(NAMESPACE, READONLY).and_then(|n| n.get_blob(KEY)) {
            Ok(d) => Some(d),
            Err(e) if e == crate::idf::raw::ESP_ERR_NVS_NOT_FOUND as EspErr => None,
            Err(e) => {
                log!(Error, TAG, "reading the profiles failed: {}", err_name(e));
                Some(Vec::new())
            }
        }
    }

    pub fn write(data: &[u8]) -> bool {
        let nvs = match Nvs::open(NAMESPACE, READWRITE) {
            Ok(n) => n,
            Err(e) => {
                log!(Error, TAG, "nvs_open failed: {}", err_name(e));
                return false;
            }
        };
        let e = nvs.set_blob(KEY, data);
        let e = if e == ESP_OK { nvs.commit() } else { e };
        if e != ESP_OK {
            log!(Error, TAG, "saving the profiles failed: {}", err_name(e));
            return false;
        }
        true
    }
}

/// On the host: kept in memory.
#[cfg(not(target_os = "espidf"))]
mod nvs {
    use std::sync::Mutex;

    pub static BLOB: Mutex<Option<Vec<u8>>> = Mutex::new(None);

    pub fn read() -> Option<Vec<u8>> {
        BLOB.lock().unwrap().clone()
    }

    pub fn write(data: &[u8]) -> bool {
        *BLOB.lock().unwrap() = Some(data.to_vec());
        true
    }
}

/// The stored profiles and the one last applied.
pub fn list() -> (Vec<Profile>, Option<usize>) {
    with(|l| (l.clone(), active(l.len())))
}

/// Replaces the profiles; false if they could not be saved. The profile
/// last applied stays marked where it moved to, or is unmarked if it was
/// deleted or changed.
pub fn store(new: Vec<Profile>) -> bool {
    with(|l| {
        if !nvs::write(&serialize(&new)) {
            return false;
        }
        log!(Info, TAG, "{} profile(s) saved", new.len());
        let active = moved(l, active(l.len()), &new);
        if config::has(ACTIVE_KEY) {
            let v = active.map_or(NONE, |i| i as u16);
            config::set(ACTIVE_KEY, Json::Num(Num::U(v as u64)));
            crate::config::nvs::request_flush();
        }
        *l = new;
        true
    })
}

/// Applies profile `id` to the config (the caller persists and reloads);
/// its name, or None if there is no such profile.
pub fn apply(id: usize, s: &mut impl crate::config::settings_apply::Store) -> Option<Vec<u8>> {
    let p = with(|l| l.get(id).cloned())?;
    let mut applied = crate::config::settings_apply::Applied::default();
    let pid_use_max = crate::config::settings_apply::pid_use_max_board();
    for w in crate::http::v2::settings::settings_v2(&p.settings) {
        crate::config::settings_apply::apply(&w, &mut applied, pid_use_max, s);
    }
    s.set_u16(ACTIVE_KEY, id as u16);
    log!(
        Info,
        TAG,
        "profile {id} applied: {}",
        String::from_utf8_lossy(&p.name)
    );
    Some(p.name)
}

// ---------------------------------------------------------------------------
// HTTP

pub mod http {
    use super::*;
    use crate::idf::httpd::{self, err, Request};
    use crate::idf::{EspErr, ESP_FAIL};

    fn send_err(req: &mut Request, code: httpd::ErrCode, msg: &CStr) -> EspErr {
        req.send_err(code, Some(msg))
    }

    fn send_doc(req: &mut Request, doc: Json) -> EspErr {
        req.set_type(c"application/json");
        crate::http::server::send_json(req, &doc.serialize())
    }

    /// The request body.
    fn body(req: &mut Request) -> Option<Vec<u8>> {
        crate::http::server::read_body(req).ok()
    }

    /// `GET /api/v2/profiles`
    pub fn get(req: &mut Request) -> EspErr {
        ensure_default(crate::http::v2::settings::v2_settings_doc);
        let (l, a) = list();
        send_doc(req, to_json(&l, a))
    }

    /// `PATCH /api/v2/profiles` (OTP): replaces all profiles.
    pub fn patch(req: &mut Request) -> EspErr {
        ensure_default(crate::http::v2::settings::v2_settings_doc);
        let Some(b) = body(req) else {
            return ESP_FAIL;
        };
        let new = match parse_patch(&b) {
            Ok(n) => n,
            Err(e) => return send_err(req, err::E400_BAD_REQUEST, e.message()),
        };
        if !store(new) {
            return send_err(
                req,
                err::E500_INTERNAL_SERVER_ERROR,
                c"Saving the profiles failed",
            );
        }
        get(req)
    }

    /// `POST /api/v2/profiles/apply` with `{"id": n}` (no OTP: it only
    /// selects a stored profile).
    pub fn apply(req: &mut Request) -> EspErr {
        ensure_default(crate::http::v2::settings::v2_settings_doc);
        let Some(b) = body(req) else {
            return ESP_FAIL;
        };
        let count = list().0.len();
        let Some(id) = parse_apply(&b, count) else {
            return send_err(req, err::E400_BAD_REQUEST, c"Unknown profile id");
        };
        let Some(name) = super::apply(id, &mut crate::config::settings_apply::Config) else {
            return send_err(req, err::E400_BAD_REQUEST, c"Unknown profile id");
        };
        crate::config::nvs::request_flush();
        let r = send_doc(
            req,
            Json::Obj(vec![
                (b"id".to_vec(), Json::Num(Num::U(id as u64))),
                (b"name".to_vec(), Json::Str(name)),
            ]),
        );
        crate::http::sources::reload_settings();
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OC: &str = r#"{"name":"OC","settings":{"frequency":1000,"coreVoltage":1350,
        "poolMode":1,"poolBalance":70,"hostname":"x","fans":[{"mode":2,"pid":{"targetTemp":60,"p":6,"x":1},"y":2}]}}"#;

    #[test]
    fn keeps_only_profile_settings() {
        let list = parse_patch(format!(r#"{{"profiles":[{OC}]}}"#).as_bytes()).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, b"OC");
        assert_eq!(
            String::from_utf8(list[0].settings.serialize()).unwrap(),
            r#"{"frequency":1000,"coreVoltage":1350,"fans":[{"mode":2,"pid":{"targetTemp":60,"p":6}}]}"#
        );
        // stored and read back unchanged
        assert_eq!(parse_stored(&serialize(&list)), list);
    }

    #[test]
    fn takes_the_fans_the_device_reports() {
        // GET /api/v2/settings of a Q1373: the second fan follows the first
        let fans = r#"[{"label":"M2","mode":2,"manualSpeed":100,"overheatTemp":70,"pid":{"targetTemp":60,"p":6,"i":0.1,"d":10}},
            {"label":"M1","mode":3,"manualSpeed":100,"overheatTemp":70,"pid":{"targetTemp":-1,"p":6,"i":0.1,"d":10}}]"#;
        let body = format!(
            r#"{{"profiles":[{{"name":"Now","settings":{{"frequency":425,"coreVoltage":1010,"poolMode":1,"poolBalance":70,"fans":{fans}}}}}]}}"#
        );
        let list = parse_patch(body.as_bytes()).unwrap();
        let fans = list[0].settings.get("fans");
        assert_eq!(fans.at(1).get("mode").as_uint(3), Some(3));
        assert!(fans.at(0).get("label").is_null());
    }

    #[test]
    fn default_profile_once() {
        let _g = crate::config::tests::LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let doc = || {
            ajson::deserialize(
                br#"{"hostname":"x","frequency":600,"coreVoltage":1150,"poolMode":0,"poolBalance":0,
                "fans":[{"label":"M2","mode":3,"manualSpeed":100,"overheatTemp":70,"pid":{"targetTemp":-1,"p":6,"i":0.1,"d":10}}]}"#,
            )
            .unwrap()
        };
        // failover: no balance
        let s = from_settings(&doc()).unwrap();
        assert!(
            s.get("poolMode").is_null()
                && s.get("poolBalance").is_null()
                && s.get("hostname").is_null()
        );
        assert_eq!(s.get("frequency").as_uint(0xffff), Some(600));
        // never stored: "default" is created and marked active
        *PROFILES.lock().unwrap() = None;
        *nvs::BLOB.lock().unwrap() = None;
        ensure_default(doc);
        let (l, a) = list();
        assert_eq!(
            (l.len(), l[0].name.as_slice(), a),
            (1, &b"default"[..], Some(0))
        );
        // emptied by the user: stays empty, also after loading again
        assert!(store(vec![]));
        *PROFILES.lock().unwrap() = None;
        ensure_default(doc);
        assert!(list().0.is_empty());
    }

    #[test]
    fn refuses_invalid_lists() {
        let p = |s: &str| parse_patch(s.as_bytes());
        assert_eq!(p("{"), Err(Error::InvalidJson));
        assert_eq!(p(r#"{"profiles":{}}"#), Err(Error::NotAList));
        let six = [r#"{"name":"a","settings":{}}"#; 6].join(",");
        assert_eq!(
            p(&format!(r#"{{"profiles":[{six}]}}"#)),
            Err(Error::TooMany)
        );
        assert_eq!(
            p(r#"{"profiles":[{"name":"","settings":{}}]}"#),
            Err(Error::BadName)
        );
        assert_eq!(
            p(&format!(
                r#"{{"profiles":[{{"name":"{}","settings":{{}}}}]}}"#,
                "n".repeat(33)
            )),
            Err(Error::BadName)
        );
        // 32 characters in 35 bytes are fine, 33 characters are not
        let ok = "Nacht – leise, Lüfter gedrosselt";
        assert_eq!((ok.chars().count(), ok.len()), (32, 35));
        assert!(p(&format!(
            r#"{{"profiles":[{{"name":"{ok}","settings":{{}}}}]}}"#
        ))
        .is_ok());
        assert_eq!(
            p(&format!(
                r#"{{"profiles":[{{"name":"{ok}ü","settings":{{}}}}]}}"#
            )),
            Err(Error::BadName)
        );
        for bad in [
            r#"{"frequency":0}"#,
            r#"{"frequency":"600"}"#,
            r#"{"fans":[{},{},{}]}"#,
            r#"{"fans":[{"manualSpeed":101}]}"#,
            r#"{"fans":[{"pid":{"p":"x"}}]}"#,
            r#"{"fans":[{"mode":4}]}"#,
            "[]",
        ] {
            assert_eq!(
                p(&format!(
                    r#"{{"profiles":[{{"name":"a","settings":{bad}}}]}}"#
                )),
                Err(Error::BadSettings),
                "{bad}"
            );
        }
        assert_eq!(p(r#"{"profiles":[]}"#), Ok(vec![]));
    }

    #[test]
    fn the_active_mark_follows_its_profile() {
        let p = |n: &str, f: u64| Profile {
            name: n.into(),
            settings: Json::Obj(vec![(b"frequency".to_vec(), Json::Num(Num::U(f)))]),
        };
        let old = [p("a", 1), p("b", 2), p("c", 3)];
        // a profile before it deleted: it moves up
        assert_eq!(moved(&old, Some(2), &[p("a", 1), p("c", 3)]), Some(1));
        // the active one deleted: nothing is marked, not its successor
        assert_eq!(moved(&old, Some(0), &[p("b", 2), p("c", 3)]), None);
        // the active one changed: no longer what was applied
        assert_eq!(
            moved(&old, Some(1), &[p("a", 1), p("b", 9), p("c", 3)]),
            None
        );
        assert_eq!(moved(&old, Some(1), &[]), None);
        assert_eq!(moved(&old, None, &old), None);
    }

    #[test]
    fn apply_id_must_exist() {
        assert_eq!(parse_apply(br#"{"id":1}"#, 2), Some(1));
        assert_eq!(parse_apply(br#"{"id":2}"#, 2), None);
        assert_eq!(parse_apply(br#"{"id":-1}"#, 2), None);
        assert_eq!(parse_apply(b"{}", 2), None);
    }

    #[derive(Default)]
    struct Rec(Vec<String>);

    impl crate::config::settings_apply::Store for Rec {
        fn set_u16(&mut self, key: &[u8], v: u16) {
            self.0.push(format!("{}={v}", String::from_utf8_lossy(key)));
        }
        fn set_u64(&mut self, key: &[u8], v: u64) {
            self.0.push(format!("{}={v}", String::from_utf8_lossy(key)));
        }
        fn set_str(&mut self, _: &[u8], _: Option<&[u8]>) {}
        fn get_u16(&mut self, _: &[u8], def: u16) -> u16 {
            def
        }
    }

    #[test]
    fn applies_through_the_settings_path() {
        let _g = crate::config::tests::LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let list = parse_patch(
            br#"{"profiles":[{"name":"Normal","settings":{"frequency":600,"coreVoltage":1120}},
                {"name":"Quiet","settings":{"poolMode":1,"poolBalance":30,"fans":[{"mode":0,"manualSpeed":40}]}}]}"#,
        )
        .unwrap();
        assert!(store(list));
        let mut r = Rec::default();
        assert_eq!(apply(1, &mut r).as_deref(), Some(&b"Quiet"[..]));
        // the pool settings are not part of a profile
        assert_eq!(r.0, ["autofanspeed=0", "fanspeed=40", "profile_act=1"]);
        let mut r = Rec::default();
        assert_eq!(apply(0, &mut r).as_deref(), Some(&b"Normal"[..]));
        assert!(
            r.0.iter().any(|w| w.ends_with("=600")) && r.0.iter().any(|w| w.ends_with("=1120"))
        );
        assert!(apply(2, &mut Rec::default()).is_none());
        let doc = to_json(&list_only(), Some(0)).serialize();
        assert!(String::from_utf8(doc)
            .unwrap()
            .starts_with(r#"{"max":5,"active":0,"profiles":[{"id":0,"name":"Normal""#));
        assert!(store(vec![]));
    }

    fn list_only() -> Vec<Profile> {
        list().0
    }
}
