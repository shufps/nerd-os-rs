//! The CAN swarm API (port of `main/http_server/handler_can_swarm.cpp`):
//! `GET /api/v2/can/nodes` (the master and its slaves) and the commands to
//! one slave, `POST /api/v2/can/nodes/{id}/{restart|shutdown|identify}`,
//! `PATCH` and `DELETE /api/v2/can/nodes/{id}`.

use crate::idf::idf_const;
use core::ffi::{c_int, CStr};
use std::ffi::CString;

use crate::can;
use crate::http_json::{CanMasterNode, FanInfo};
use crate::http_query::{can_node_action, can_node_id, NodeAction, NodeId};
use crate::http_settings::Setting;
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

use crate::board::ffi::info as board_info;

/// `GET /api/v2/can/nodes`
pub fn nodes(req: &mut Request) -> EspErr {
    req.set_type(c"application/json");
    let b = board_info();
    let version = CString::new(crate::mdns::fw_version()).unwrap_or_default();
    let temps: Vec<f32> = (0..b.asic_count.max(0))
        .map(|j| crate::board_core::CHIP_TEMPS.get(j))
        .collect();

    let mut fans = [FanInfo::zeroed(); 2];
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
        mac: crate::network::mac_addr().as_ptr(),
        device_model: b.device_model,
        version: version.as_ptr(),
        shutdown: power_mgmt::is_shutdown(),
        hashrate: crate::system::current_hashrate(),
        power: power_mgmt::get(0),
        temp: power_mgmt::get(3),
        vr_temp: power_mgmt::get(4),
        current: power_mgmt::get(2),
        vout: crate::board::ffi::board_power_value(3),
        board_error: crate::system::error().0,
        free_heap_int: crate::idf::task::free_heap(CAP_INTERNAL) as c_int,
        frequency: b.asic_frequency,
        core_voltage: b.asic_voltage_mv,
        flip_screen: b.flip_screen,
        auto_screen_off: crate::cfg::auto_screen_off(),
        asic_count: temps.len() as c_int,
        asic_temps: temps.as_ptr(),
        fan_count: b.num_fans,
        fans,
    };
    crate::http_server::send_json(req, &crate::http_json::can_swarm(&m).serialize())
}

/// `PATCH /api/v2/can/nodes/{id}`: frequency, voltage, fans, display
/// flags (the field names of the info PATCH).
pub fn patch(req: &mut Request) -> EspErr {
    let slave = match node_id(req) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let Ok(body) = crate::http_server::read_body(req) else {
        return ESP_FAIL;
    };
    let Ok(doc) = crate::ajson::deserialize(body) else {
        log!(Error, c"can_swarm", "JSON parsing failed");
        send_err(req, err::E400_BAD_REQUEST, c"Invalid JSON");
        return ESP_FAIL;
    };
    for w in crate::http_settings::can_node(&doc) {
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
