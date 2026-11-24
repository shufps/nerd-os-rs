//! ESP-IDF HTTP client (`esp_http_client.h`): the configuration struct
//! from the generated bindings, requests with the firmware's
//! certificate bundle (`crt_bundle_attach`).

use super::idf_const;
use core::ffi::{c_char, c_int, c_void, CStr};

use super::{EspErr, ESP_OK};

/// `esp_http_client_method_t` (enums: `u32` in the bindings)
pub type Method = u32;
idf_const!(pub METHOD_GET: Method = esp_http_client_method_t_HTTP_METHOD_GET, 0);
idf_const!(pub METHOD_POST: Method = esp_http_client_method_t_HTTP_METHOD_POST, 1);

#[cfg(target_os = "espidf")]
pub use crate::idf::raw::{esp_http_client_config_t as Config, esp_http_client_event_t as Event};
#[cfg(target_os = "espidf")]
pub type Handle = crate::idf::raw::esp_http_client_handle_t;

/// `esp_http_client_config_t` (host model)
#[cfg(not(target_os = "espidf"))]
#[derive(Clone, Copy)]
pub struct Config {
    pub url: *const c_char,
    pub host: *const c_char,
    pub port: c_int,
    pub username: *const c_char,
    pub password: *const c_char,
    pub auth_type: c_int,
    pub path: *const c_char,
    pub query: *const c_char,
    pub cert_pem: *const c_char,
    pub cert_len: usize,
    pub client_cert_pem: *const c_char,
    pub client_cert_len: usize,
    pub client_key_pem: *const c_char,
    pub client_key_len: usize,
    pub client_key_password: *const c_char,
    pub client_key_password_len: usize,
    pub tls_version: c_int,
    pub user_agent: *const c_char,
    pub method: Method,
    pub timeout_ms: c_int,
    pub disable_auto_redirect: bool,
    pub max_redirection_count: c_int,
    pub max_authorization_retries: c_int,
    pub event_handler: Option<EventHandler>,
    pub transport_type: c_int,
    pub buffer_size: c_int,
    pub buffer_size_tx: c_int,
    pub user_data: *mut c_void,
    pub is_async: bool,
    pub use_global_ca_store: bool,
    pub skip_cert_common_name_check: bool,
    pub common_name: *const c_char,
    pub crt_bundle_attach: Option<unsafe extern "C" fn(*mut c_void) -> EspErr>,
    pub keep_alive_enable: bool,
    pub keep_alive_idle: c_int,
    pub keep_alive_interval: c_int,
    pub keep_alive_count: c_int,
    pub if_name: *mut c_void,
    pub addr_type: c_int,
}

#[cfg(not(target_os = "espidf"))]
impl Default for Config {
    fn default() -> Self {
        // SAFETY: all fields are raw pointers, integers, bools or `Option<fn>`, for which
        // all zero is valid (null / 0 / false / None), as `esp_http_client_config_t config = {};`
        unsafe { core::mem::zeroed() }
    }
}

impl Config {
    /// All zero with the firmware's certificate bundle.
    pub fn with_bundle() -> Self {
        Config {
            crt_bundle_attach: Some(crate::security::crt_bundle::attach),
            ..Config::default()
        }
    }
}

#[cfg(not(target_os = "espidf"))]
pub type Handle = *mut c_void;

#[cfg(target_os = "espidf")]
mod sys {
    pub use crate::idf::raw::{
        esp_http_client_cleanup, esp_http_client_close, esp_http_client_fetch_headers,
        esp_http_client_get_content_length, esp_http_client_get_errno,
        esp_http_client_get_status_code, esp_http_client_get_transport_type, esp_http_client_init,
        esp_http_client_open, esp_http_client_perform, esp_http_client_read,
        esp_http_client_read_response, esp_http_client_set_header, esp_http_client_set_method,
        esp_http_client_set_post_field, esp_http_client_set_url, esp_http_client_write,
    };
}

#[cfg(not(target_os = "espidf"))]
mod sys {
    use super::*;
    use crate::idf::ESP_FAIL;
    pub unsafe fn esp_http_client_init(_: *const Config) -> Handle {
        core::ptr::null_mut()
    }
    pub unsafe fn esp_http_client_set_url(_: Handle, _: *const c_char) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn esp_http_client_read(_: Handle, _: *mut c_char, _: c_int) -> c_int {
        -1
    }
    pub unsafe fn esp_http_client_set_method(_: Handle, _: Method) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn esp_http_client_set_header(
        _: Handle,
        _: *const c_char,
        _: *const c_char,
    ) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn esp_http_client_set_post_field(_: Handle, _: *const c_char, _: c_int) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn esp_http_client_perform(_: Handle) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn esp_http_client_get_status_code(_: Handle) -> c_int {
        -1
    }
    pub unsafe fn esp_http_client_cleanup(_: Handle) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn esp_http_client_open(_: Handle, _: c_int) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn esp_http_client_write(_: Handle, _: *const c_char, _: c_int) -> c_int {
        -1
    }
    pub unsafe fn esp_http_client_fetch_headers(_: Handle) -> i64 {
        -1
    }
    pub unsafe fn esp_http_client_read_response(_: Handle, _: *mut c_char, _: c_int) -> c_int {
        -1
    }
    pub unsafe fn esp_http_client_get_content_length(_: Handle) -> i64 {
        -1
    }
    pub unsafe fn esp_http_client_get_errno(_: Handle) -> c_int {
        0
    }
    pub unsafe fn esp_http_client_get_transport_type(_: Handle) -> u32 {
        0
    }
    pub unsafe fn esp_http_client_close(_: Handle) -> EspErr {
        ESP_FAIL
    }
}

idf_const!(pub TRANSPORT_OVER_SSL: u32 = esp_http_client_transport_t_HTTP_TRANSPORT_OVER_SSL, 2);

/// `esp_http_client_event_t` (host model)
#[cfg(not(target_os = "espidf"))]
#[repr(C)]
pub struct Event {
    pub event_id: u32,
    pub client: Handle,
    pub data: *mut c_void,
    pub data_len: c_int,
    pub user_data: *mut c_void,
    pub header_key: *mut c_char,
    pub header_value: *mut c_char,
}

idf_const!(pub EVENT_ERROR: u32 = esp_http_client_event_id_t_HTTP_EVENT_ERROR, 0);
idf_const!(pub EVENT_ON_DATA: u32 = esp_http_client_event_id_t_HTTP_EVENT_ON_DATA, 4);
idf_const!(pub EVENT_DISCONNECTED: u32 = esp_http_client_event_id_t_HTTP_EVENT_DISCONNECTED, 6);

/// `http_event_handle_cb`
pub type EventHandler = unsafe extern "C" fn(*mut Event) -> EspErr;

/// A client, cleaned up on drop.
pub struct Client(Handle);

impl Client {
    /// A client for `url` with the certificate bundle; None if the
    /// initialization failed.
    pub fn new(url: &CStr, method: Method, timeout_ms: c_int) -> Option<Client> {
        let config = Config {
            url: url.as_ptr(),
            method,
            timeout_ms,
            crt_bundle_attach: Some(crate::security::crt_bundle::attach),
            ..Config::default()
        };
        // SAFETY: `config` is a valid local and `url` a NUL-terminated string; init copies the strings it keeps.
        let h = unsafe { sys::esp_http_client_init(&config) };
        (!h.is_null()).then_some(Client(h))
    }

    /// A client for `config`; None if the initialization failed.
    pub fn from_config(config: &Config) -> Option<Client> {
        // SAFETY: `config` is a valid reference for the call; init copies the strings it keeps.
        let h = unsafe { sys::esp_http_client_init(config) };
        (!h.is_null()).then_some(Client(h))
    }

    pub fn set_url(&self, url: &CStr) -> EspErr {
        // SAFETY: the handle comes from a successful `esp_http_client_init` and is only freed in `drop`; `url` is NUL-terminated and copied.
        unsafe { sys::esp_http_client_set_url(self.0, url.as_ptr()) }
    }

    /// Reads body bytes into `buf` (`esp_http_client_read()`): the bytes
    /// read, 0 at the end, < 0 on an error.
    pub fn read(&self, buf: &mut [u8]) -> c_int {
        // SAFETY: the handle comes from a successful `esp_http_client_init` and is only freed in `drop`; at most `buf.len()` bytes are written to `buf`.
        unsafe {
            sys::esp_http_client_read(self.0, buf.as_mut_ptr() as *mut c_char, buf.len() as c_int)
        }
    }

    /// A GET client for `url` with an event handler and its context.
    pub fn with_handler(
        url: &CStr,
        handler: EventHandler,
        user_data: *mut c_void,
    ) -> Option<Client> {
        let config = Config {
            url: url.as_ptr(),
            event_handler: Some(handler),
            user_data,
            crt_bundle_attach: Some(crate::security::crt_bundle::attach),
            ..Config::default()
        };
        // SAFETY: `config` is a valid local and `url` NUL-terminated (copied); the caller keeps
        // `user_data` valid for what `handler` does with it.
        let h = unsafe { sys::esp_http_client_init(&config) };
        (!h.is_null()).then_some(Client(h))
    }

    /// The whole request (redirects followed); the body goes to the handler.
    pub fn perform(&self) -> EspErr {
        // SAFETY: the handle comes from a successful `esp_http_client_init` and is only freed in `drop`.
        unsafe { sys::esp_http_client_perform(self.0) }
    }

    pub fn set_method(&self, method: Method) -> EspErr {
        // SAFETY: the handle comes from a successful `esp_http_client_init` and is only freed in `drop`.
        unsafe { sys::esp_http_client_set_method(self.0, method) }
    }

    pub fn set_header(&self, key: &CStr, value: &CStr) -> EspErr {
        // SAFETY: the handle comes from a successful `esp_http_client_init` and is only freed in `drop`; key and value are NUL-terminated and copied.
        unsafe { sys::esp_http_client_set_header(self.0, key.as_ptr(), value.as_ptr()) }
    }

    /// Sends the request with `body` (kept by the client until `perform`).
    pub fn perform_with_body(&self, body: &[u8]) -> EspErr {
        // SAFETY: the handle comes from a successful `esp_http_client_init` and is
        // only freed in `drop`; the client keeps the `body` pointer (not a copy):
        // it is borrowed until `perform` returns and cleared right after, so a
        // later request cannot read it
        unsafe {
            let e = sys::esp_http_client_set_post_field(
                self.0,
                body.as_ptr() as *const c_char,
                body.len() as c_int,
            );
            if e != ESP_OK {
                return e;
            }
            let e = sys::esp_http_client_perform(self.0);
            sys::esp_http_client_set_post_field(self.0, core::ptr::null(), 0);
            e
        }
    }

    pub fn status(&self) -> c_int {
        // SAFETY: the handle comes from a successful `esp_http_client_init` and is only freed in `drop`.
        unsafe { sys::esp_http_client_get_status_code(self.0) }
    }

    /// Connects and sends the request line and headers; `write_len` bytes
    /// of body follow with `write()`.
    pub fn open(&self, write_len: usize) -> EspErr {
        // SAFETY: the handle comes from a successful `esp_http_client_init` and is only freed in `drop`.
        unsafe { sys::esp_http_client_open(self.0, write_len as c_int) }
    }

    /// The bytes written, < 0 on an error.
    pub fn write(&self, data: &[u8]) -> c_int {
        // SAFETY: the handle comes from a successful `esp_http_client_init` and is only freed in `drop`; `data.len()` bytes are read from `data`.
        unsafe {
            sys::esp_http_client_write(self.0, data.as_ptr() as *const c_char, data.len() as c_int)
        }
    }

    /// The content length (-1 if chunked or unknown, < 0 on an error).
    pub fn fetch_headers(&self) -> i64 {
        // SAFETY: the handle comes from a successful `esp_http_client_init` and is only freed in `drop`.
        unsafe { sys::esp_http_client_fetch_headers(self.0) }
    }

    /// Reads the response body (also chunked) into `buf`; the bytes read.
    pub fn read_response(&self, buf: &mut [u8]) -> c_int {
        // SAFETY: the handle comes from a successful `esp_http_client_init` and is only freed in `drop`; at most `buf.len()` bytes are written to `buf`.
        unsafe {
            sys::esp_http_client_read_response(
                self.0,
                buf.as_mut_ptr() as *mut c_char,
                buf.len() as c_int,
            )
        }
    }

    pub fn content_length(&self) -> i64 {
        // SAFETY: the handle comes from a successful `esp_http_client_init` and is only freed in `drop`.
        unsafe { sys::esp_http_client_get_content_length(self.0) }
    }

    /// The errno of the client of an event.
    ///
    /// # Safety
    /// `h` a live client handle.
    pub unsafe fn errno_of(h: Handle) -> c_int {
        // SAFETY: the caller guarantees `h` is a live client handle.
        unsafe { sys::esp_http_client_get_errno(h) }
    }

    /// # Safety
    /// `h` a live client handle.
    pub unsafe fn is_ssl_of(h: Handle) -> bool {
        // SAFETY: the caller guarantees `h` is a live client handle.
        unsafe { sys::esp_http_client_get_transport_type(h) == TRANSPORT_OVER_SSL }
    }

    pub fn errno(&self) -> c_int {
        // SAFETY: the handle comes from a successful `esp_http_client_init` and is only freed in `drop`.
        unsafe { sys::esp_http_client_get_errno(self.0) }
    }

    pub fn is_ssl(&self) -> bool {
        // SAFETY: the handle comes from a successful `esp_http_client_init` and is only freed in `drop`.
        unsafe { sys::esp_http_client_get_transport_type(self.0) == TRANSPORT_OVER_SSL }
    }

    pub fn close(&self) -> EspErr {
        // SAFETY: the handle comes from a successful `esp_http_client_init` and is only freed in `drop`.
        unsafe { sys::esp_http_client_close(self.0) }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // SAFETY: the handle is live and owned by this `Client`; it is not used after drop.
        unsafe { sys::esp_http_client_cleanup(self.0) };
    }
}
