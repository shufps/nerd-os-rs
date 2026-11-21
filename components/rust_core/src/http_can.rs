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
use crate::idf::httpd::{self, err, Req};
use crate::idf::{EspErr, ESP_FAIL, ESP_OK};
use crate::log::log;
use crate::power_mgmt;

idf_const!(CAP_INTERNAL: u32 = MALLOC_CAP_INTERNAL, 1 << 11);

unsafe fn send_err(req: *mut Req, code: httpd::ErrCode, msg: &CStr) -> EspErr {
    httpd::httpd_resp_send_err(req, code, msg.as_ptr())
}

unsafe fn send_ok(req: *mut Req) -> EspErr {
    httpd::httpd_resp_send(req, c"{\"ok\":true}".as_ptr(), httpd::RESP_USE_STRLEN);
    ESP_OK
}

/// The node ID of `/api/v2/can/nodes/{id}`, or the 400 sent.
unsafe fn node_id(req: *mut Req) -> Result<u8, EspErr> {
    let uri = CStr::from_ptr((*req).uri.as_ptr()).to_bytes();
    match can_node_id(uri) {
        NodeId::Id(id) => Ok(id),
        NodeId::Missing => Err(send_err(req, err::E400_BAD_REQUEST, c"missing id")),
        NodeId::Invalid => Err(send_err(req, err::E400_BAD_REQUEST, c"invalid id")),
    }
}

fn board_info() -> crate::board::ffi::Info {
    let mut i = core::mem::MaybeUninit::uninit();
    unsafe {
        crate::board::ffi::board_info(i.as_mut_ptr());
        i.assume_init()
    }
}

/// `GET /api/v2/can/nodes`
///
/// # Safety
/// `req` valid.
pub unsafe extern "C" fn nodes(req: *mut Req) -> EspErr {
    httpd::httpd_resp_set_type(req, c"application/json".as_ptr());
    let b = board_info();
    let version = CString::new(crate::mdns::fw_version()).unwrap_or_default();
    let temps: Vec<f32> = (0..b.asic_count.max(0))
        .map(|j| crate::board_core::CHIP_TEMPS.get(j))
        .collect();

    let mut fans = [FanInfo {
        label: core::ptr::null(),
        mode: 0,
        manual_speed: 0,
        overheat_temp: 0,
        rpm: 0,
        speed_perc: 0,
        target_temp: 0,
        p: 0,
        i: 0,
        d: 0,
    }; 2];
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
    let mut len = 0;
    let json = crate::cutil::malloc_copy(&crate::http_json::can_swarm(&m).serialize(), &mut len);
    crate::http_server::send_json(req, json, len)
}

/// `PATCH /api/v2/can/nodes/{id}`: frequency, voltage, fans, display
/// flags (the field names of the info PATCH).
///
/// # Safety
/// `req` valid, its context the server context.
pub unsafe extern "C" fn patch(req: *mut Req) -> EspErr {
    let slave = match node_id(req) {
        Ok(id) => id,
        Err(e) => return e,
    };
    if crate::http_server::post_data(req) != ESP_OK {
        return ESP_FAIL;
    }
    let ctx = (*req).user_ctx as *const crate::http_server::ServerCtx;
    let body = CStr::from_ptr((*ctx).scratch.as_ptr()).to_bytes();
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
///
/// # Safety
/// `req` valid.
pub unsafe extern "C" fn action(req: *mut Req) -> EspErr {
    let uri = CStr::from_ptr((*req).uri.as_ptr()).to_bytes();
    let (id, action) = can_node_action(uri);
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
///
/// # Safety
/// `req` valid.
pub unsafe extern "C" fn delete(req: *mut Req) -> EspErr {
    match node_id(req) {
        Ok(id) => {
            can::delete_slave(id);
            send_ok(req)
        }
        Err(e) => e,
    }
}
