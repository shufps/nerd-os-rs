//! The CAN swarm API (port of `main/http_server/handler_can_swarm.cpp`):
//! `GET /api/v2/can/nodes` (the master and its slaves) and the commands to
//! one slave, `POST /api/v2/can/nodes/{id}/{restart|shutdown|identify}`,
//! `PATCH` and `DELETE /api/v2/can/nodes/{id}`.

use crate::idf::idf_const;
use core::ffi::{c_int, CStr};
use std::ffi::CString;

use crate::ajson::Json;
use crate::can;
use crate::http::json::{float, FanInfo, Obj};
use crate::http::query::{can_node_action, can_node_id, NodeAction, NodeId};
use crate::http::settings::{field, uint, Out, Setting, Setting as S, Write, NULL};
use crate::idf::httpd::{self, err, Request};
use crate::idf::{EspErr, ESP_FAIL, ESP_OK};
use crate::log::log;
use crate::power_mgmt;

idf_const!(CAP_INTERNAL: u32 = MALLOC_CAP_INTERNAL, 1 << 11);

fn send_err(req: &mut Request, code: httpd::ErrCode, msg: &CStr) -> EspErr {
    req.send_err(code, Some(msg))
}

fn send_ok(req: &mut Request) -> EspErr {
    req.send(b"{\"ok\":true}");
    ESP_OK
}

/// The node ID of `/api/v2/can/nodes/{id}`, or the 400 sent.
fn node_id(req: &mut Request) -> Result<u8, EspErr> {
    match can_node_id(req.uri().to_bytes()) {
        NodeId::Id(id) => Ok(id),
        NodeId::Missing => Err(send_err(req, err::E400_BAD_REQUEST, c"missing id")),
        NodeId::Invalid => Err(send_err(req, err::E400_BAD_REQUEST, c"invalid id")),
    }
}

use crate::board::api::info as board_info;

/// `GET /api/v2/can/nodes`
pub fn nodes(req: &mut Request) -> EspErr {
    req.set_type(c"application/json");
    let b = board_info();
    let version = CString::new(crate::mdns::fw_version()).unwrap_or_default();
    let temps: Vec<f32> = (0..b.asic_count.max(0))
        .map(|j| crate::board::board_core::CHIP_TEMPS.get(j))
        .collect();

    let mut fans: [FanInfo; 2] = Default::default();
    for (ch, f) in fans.iter_mut().enumerate() {
        f.rpm = power_mgmt::fan_get(ch as c_int, 0);
        f.speed_perc = power_mgmt::fan_get(ch as c_int, 1);
        if (ch as c_int) < b.num_fans {
            f.mode = b.fan_mode[ch];
            f.manual_speed = crate::cfg::fan_manual_speed(ch);
            f.overheat_temp = crate::cfg::fan_overheat_temp(ch);
            f.target_temp = b.pid[ch].target_temp;
        }
    }
    let m = CanMasterNode {
        mac: Some(crate::network::mac_addr().into()),
        device_model: Some(b.device_model.into()),
        version: Some(version),
        shutdown: power_mgmt::is_shutdown(),
        hashrate: crate::system::current_hashrate(),
        power: power_mgmt::get(0),
        temp: power_mgmt::get(3),
        vr_temp: power_mgmt::get(4),
        current: power_mgmt::get(2),
        vout: crate::board::api::board_power_value(3),
        board_error: crate::system::error().0,
        free_heap_int: crate::idf::task::free_heap(CAP_INTERNAL) as c_int,
        frequency: b.asic_frequency,
        core_voltage: b.asic_voltage_mv,
        flip_screen: b.flip_screen,
        auto_screen_off: crate::cfg::auto_screen_off(),
        asic_temps: temps,
        fan_count: b.num_fans,
        fans,
    };
    let doc = can_swarm(&m);
    crate::http::server::send_json(req, &doc.serialize())
}

/// `PATCH /api/v2/can/nodes/{id}`: frequency, voltage, fans, display
/// flags (the field names of the info PATCH).
pub fn patch(req: &mut Request) -> EspErr {
    let slave = match node_id(req) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let Ok(body) = crate::http::server::read_body(req) else {
        return ESP_FAIL;
    };
    let Ok(doc) = crate::ajson::deserialize(&body) else {
        log!(Error, c"can_swarm", "JSON parsing failed");
        send_err(req, err::E400_BAD_REQUEST, c"Invalid JSON");
        return ESP_FAIL;
    };
    for w in can_node(&doc) {
        let n = w.num;
        match w.setting {
            Setting::CanFrequency => can::set_slave_freq(slave, n as u16),
            Setting::CanVoltage => can::set_slave_voltage(slave, n as u16),
            Setting::CanFan => can::set_slave_fan(
                slave,
                w.index as u8,
                n as u8,
                (n >> 8) as u8,
                (n >> 16) as u8,
                (n >> 24) as u8,
            ),
            Setting::CanDisplay => {
                can::set_slave_display(slave, (n & 1) as u8, ((n >> 1) & 1) as u8)
            }
            _ => {}
        }
    }
    send_ok(req)
}

/// `POST /api/v2/can/nodes/{id}/{action}` (httpd wildcards only at the
/// end: one handler for the actions).
pub fn action(req: &mut Request) -> EspErr {
    let (id, action) = can_node_action(req.uri().to_bytes());
    let NodeId::Id(slave) = id else {
        return send_err(req, err::E400_BAD_REQUEST, c"invalid id");
    };
    match action {
        Some(NodeAction::Restart) => can::restart_slave(slave),
        Some(NodeAction::Shutdown) => can::shutdown_slave(slave),
        Some(NodeAction::Identify) => can::identify_slave(slave),
        None => return send_err(req, err::E400_BAD_REQUEST, c"unknown action"),
    }
    send_ok(req)
}

/// `DELETE /api/v2/can/nodes/{id}`: the slave out of the registry.
pub fn delete(req: &mut Request) -> EspErr {
    match node_id(req) {
        Ok(id) => {
            can::delete_slave(id);
            send_ok(req)
        }
        Err(e) => e,
    }
}

/// `can_master_node_t`: the master board's node of `GET /api/v2/can/swarm`.
#[derive(Clone, Debug, Default)]
pub struct CanMasterNode {
    pub mac: Option<CString>,
    pub device_model: Option<CString>,
    pub version: Option<CString>,
    pub shutdown: bool,
    /// `getCurrentHashrate()` (0 in a shutdown)
    pub hashrate: f32,
    pub power: f32,
    pub temp: f32,
    pub vr_temp: f32,
    /// mA
    pub current: f32,
    pub vout: f32,
    pub board_error: c_int,
    pub free_heap_int: c_int,
    pub frequency: c_int,
    pub core_voltage: c_int,
    pub flip_screen: bool,
    pub auto_screen_off: bool,
    pub asic_temps: Vec<f32>,
    pub fan_count: c_int,
    /// label/p/i/d unused
    pub fans: [FanInfo; 2],
}

/// `GET /api/v2/can/swarm`: the fleet totals, the master and the CAN slaves
/// (from the CAN master's registry).
pub fn can_swarm(m: &CanMasterNode) -> Json {
    use crate::can_master::api as can;
    use crate::can_master::{slave_config, slave_mac, slave_telemetry};
    use crate::can_master::{CONFIG_LEN, SLAVE_MAX, TELEMETRY_LEN};

    let f32_at = |b: &[u8], at: usize| f32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let u16_at = |b: &[u8], at: usize| u16::from_le_bytes([b[at], b[at + 1]]);
    let u32_at = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());

    let master_hashrate = if m.shutdown { 0.0f32 } else { m.hashrate };
    let mut slave_hashrate = 0.0f32;
    for i in 1..SLAVE_MAX as u8 {
        if !can::can_master_is_slave_known(i) {
            continue;
        }
        if let Some(t) = slave_telemetry(i).filter(|t| t[43] == 0) {
            slave_hashrate += f32_at(&t, 39);
        }
    }
    let mut fleet = Obj::new();
    fleet
        .float("hashRate", master_hashrate + slave_hashrate)
        .float("power", m.power + can::can_master_get_slave_fleet_power());

    let mut nodes = Vec::new();
    let fan_count = m.fan_count.clamp(0, 2) as usize;
    let mut master = Obj::new();
    master
        .int("id", 0)
        .cstr("mac", m.mac.as_deref())
        .bool("active", true)
        .bool("foreign", false)
        .bool("isMaster", true)
        .cstr("deviceModel", m.device_model.as_deref())
        .cstr("version", m.version.as_deref())
        .float("hashRate", master_hashrate)
        .float("temp", m.temp)
        .float("vrTemp", m.vr_temp)
        .float("power", m.power)
        .int("current", m.current as i32 as i64)
        .int("coreVoltageActual", (m.vout * 1000.0) as i32 as i64)
        .uint("fanRpm", m.fans[0].rpm as u64)
        .uint(
            "fanRpm2",
            if m.fan_count > 1 { m.fans[1].rpm } else { 0 } as u64,
        )
        .uint("fanSpeed", m.fans[0].speed_perc as u64)
        .uint(
            "fanSpeed2",
            if m.fan_count > 1 {
                m.fans[1].speed_perc
            } else {
                0
            } as u64,
        )
        .bool("shutdown", m.shutdown)
        .int("boardError", m.board_error as i64)
        .int("freeHeapInt", m.free_heap_int as i64)
        .int("frequency", m.frequency as i64)
        .int("coreVoltage", m.core_voltage as i64)
        .bool("flipScreen", m.flip_screen)
        .bool("autoScreenOff", m.auto_screen_off);
    master.arr(
        "asicTemps",
        m.asic_temps.iter().copied().map(float).collect(),
    );
    let fans = m.fans[..fan_count]
        .iter()
        .map(|f| {
            let mut o = Obj::new();
            o.int("mode", f.mode as i64)
                .uint("manualSpeed", f.manual_speed as u64)
                .uint("overheatTemp", f.overheat_temp as u64)
                .uint("targetTemp", f.target_temp as u64)
                .uint("rpm", f.rpm as u64)
                .uint("speedPerc", f.speed_perc as u64);
            o.into()
        })
        .collect();
    master.arr("fans", fans);
    nodes.push(master.into());

    for i in 1..SLAVE_MAX as u8 {
        if !can::can_master_is_slave_known(i) {
            continue;
        }
        let mac = slave_mac(i).unwrap_or_default();
        let (t, c) = (slave_telemetry(i), slave_config(i));
        let (has_telem, has_config) = (t.is_some(), c.is_some());
        let t = t.unwrap_or([0; TELEMETRY_LEN]);
        let c = c.unwrap_or([0; CONFIG_LEN]);
        let text = |b: &[u8]| {
            let b = if has_config { b } else { &[][..] };
            Json::Str(b[..b.iter().position(|&x| x == 0).unwrap_or(b.len())].to_vec())
        };
        let mac_str = mac
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":");
        let tf = |at: usize| if has_telem { f32_at(&t, at) } else { 0.0 };
        let tu16 = |at: usize| if has_telem { u16_at(&t, at) as u64 } else { 0 };
        let tu8 = |at: usize| if has_telem { t[at] as u64 } else { 0 };
        let cu16 = |at: usize| if has_config { u16_at(&c, at) as u64 } else { 0 };
        let cu8 = |at: usize| if has_config { c[at] as u64 } else { 0 };
        let mut node = Obj::new();
        node.int("id", i as i64)
            .set("mac", Json::Str(mac_str.into_bytes()))
            .bool("active", can::can_master_is_slave_active(i))
            .bool("foreign", can::can_master_is_slave_foreign(i))
            .set("deviceModel", text(&c[0..16]))
            .set("version", text(&c[16..32]))
            .float("hashRate", tf(39))
            .float("temp", tf(1))
            .float("vrTemp", tf(5))
            .float("power", tf(31))
            .uint("current", tu16(35))
            .uint("coreVoltageActual", tu16(37))
            .uint("fanRpm", tu16(25))
            .uint("fanRpm2", tu16(27))
            .uint("fanSpeed", tu8(29))
            .uint("fanSpeed2", tu8(30))
            .bool("shutdown", has_telem && t[43] != 0)
            .uint("boardError", tu8(44))
            .uint(
                "freeHeapInt",
                if has_telem { u32_at(&t, 45) as u64 } else { 0 },
            )
            .uint("frequency", cu16(32))
            .uint("coreVoltage", cu16(34))
            .bool("flipScreen", has_config && c[44] != 0)
            .bool("autoScreenOff", has_config && c[45] != 0);
        node.arr("asicTemps", (0..4).map(|j| float(tf(9 + 4 * j))).collect());
        let fans = (0..2)
            .map(|ch| {
                let b = 36 + 4 * ch;
                let mut f = Obj::new();
                f.uint("mode", cu8(b))
                    .uint("manualSpeed", cu8(b + 1))
                    .uint("overheatTemp", cu8(b + 3))
                    .uint("targetTemp", cu8(b + 2))
                    .uint("rpm", tu16(if ch == 0 { 25 } else { 27 }))
                    .uint("speedPerc", tu8(if ch == 0 { 29 } else { 30 }));
                f.into()
            })
            .collect();
        node.arr("fans", fans);
        nodes.push(node.into());
    }

    let mut d = Obj::new();
    d.obj("fleet", fleet).arr("nodes", nodes);
    d.into()
}

/// `PATCH /api/v2/can/nodes/{id}`: frequency, voltage, a fan command per
/// entry of `fans` (at most two; values converted like `as<uint8_t>()`, so
/// missing ones are 0), then the display flags if either is a bool.
pub fn can_node(doc: &Json) -> Vec<Write> {
    let mut out = Out(Vec::new());
    let f = |k| field(doc, k);
    out.num(S::CanFrequency, 0, uint(f("frequency"), 0xffff));
    out.num(S::CanVoltage, 0, uint(f("coreVoltage"), 0xffff));
    if let Some(fans) = f("fans").as_array() {
        for (ch, fan) in fans.iter().take(2).enumerate() {
            let fan = if fan.is_object() { fan } else { &NULL };
            let b = |k| field(fan, k).to_u8() as u32;
            let num =
                b("mode") | b("manualSpeed") << 8 | b("targetTemp") << 16 | b("overheatTemp") << 24;
            out.num(S::CanFan, ch as i32, Some(num));
        }
    }
    let (flip, auto_off) = (f("flipScreen"), f("autoScreenOff"));
    if flip.as_bool().is_some() || auto_off.as_bool().is_some() {
        out.num(
            S::CanDisplay,
            0,
            Some(flip.truthy() as u32 | (auto_off.truthy() as u32) << 1),
        );
    }
    out.0
}
