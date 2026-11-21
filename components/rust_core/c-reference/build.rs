//! Compiles the former C/C++ implementations for the host, with stubs for the
//! ESP-IDF headers they include. Symbols that also exist in rust_core are
//! renamed so both can live in the same test binary.

const SV2_FUNCTIONS: &[&str] = &[
    "sv2_parse_frame_header",
    "sv2_encode_frame_header",
    "sv2_build_setup_connection",
    "sv2_build_open_standard_mining_channel",
    "sv2_build_submit_shares_standard",
    "sv2_parse_setup_connection_success",
    "sv2_parse_open_channel_success",
    "sv2_parse_new_mining_job",
    "sv2_parse_set_new_prev_hash",
    "sv2_parse_set_extranonce_prefix",
    "sv2_parse_set_target",
    "sv2_parse_submit_shares_success",
    "sv2_parse_submit_shares_error",
    "sv2_build_open_extended_mining_channel",
    "sv2_build_submit_shares_extended",
    "sv2_parse_open_extended_channel_success",
    "sv2_parse_new_extended_mining_job",
    "sv2_ext_job_free",
    "sv2_target_to_pdiff",
];

const NOISE_FUNCTIONS: &[&str] = &[
    "sv2_noise_create",
    "sv2_noise_destroy",
    "sv2_noise_handshake",
    "sv2_noise_send",
    "sv2_noise_recv",
];

fn main() {
    // coinbase decoder (C)
    cc::Build::new()
        .files(["c/coinbase_decoder.c", "c/base58.c", "c/segwit_addr.c"])
        .include("shim")
        .include("c")
        .include("../include")
        .define("coinbase_process", "c_coinbase_process")
        .warnings(false)
        .compile("coinbase_c_reference");

    let cpp = || {
        let mut b = cc::Build::new();
        b.cpp(true)
            .std("c++17")
            .warnings(false)
            .include("cpp-shim")
            .include("shim");
        b
    };

    // Stratum v1: the former StratumApi (C++ with ArduinoJson) ...
    cpp()
        .files(["cpp/stratum_api.cpp", "cpp-shim/wrap.cpp"])
        .include("cpp")
        .include("arduinojson")
        .define("StratumApi", "RefStratumApi")
        .define("PREFIX", "ref_")
        .flag("-include")
        .flag("cpp-shim/host_shim.h")
        .flag("-include")
        .flag("cpp-shim/ref_shim.h")
        .compile("stratum_v1_reference");

    // ... and the Rust API that replaced it, used as the firmware does.
    cpp()
        .file("cpp-shim/wrap_current.cpp")
        .include("main-shim")
        .include("../include")
        .flag("-include")
        .flag("cpp-shim/host_shim.h")
        .compile("stratum_v1_current");

    // Job construction and nonce check: the former mining.cpp/mining_utils.cpp
    // (c-mining/, also providing hex2bin/bin2hex for the coinbase decoder) and
    // a verbatim copy of MiningInfoV1::buildBmJob().
    cpp()
        .files([
            "c-mining/mining.cpp",
            "c-mining/mining_utils.cpp",
            "cpp-shim/ref_jobs.cpp",
        ])
        .include("c-mining")
        .include("main-shim")
        .include("../include")
        // ESP-IDF headers pull in <stdlib.h> (exit), host headers do not.
        .flag("-include")
        .flag("cstdlib")
        .compile("mining_reference");

    // Stratum v2 codec (C), every function renamed to ref_sv2_*.
    let mut sv2 = cc::Build::new();
    sv2.file("c-sv2/sv2_protocol.c")
        .include("c-sv2")
        .warnings(false);
    for f in SV2_FUNCTIONS {
        sv2.define(f, Some(format!("ref_{f}").as_str()));
    }
    sv2.compile("sv2_protocol_reference");

    // Stratum v2 Noise (C), functions renamed to ref_sv2_noise_*. It calls
    // libsecp256k1 and the esp_* stand-ins of rust_core's host build, and the
    // mbedTLS stand-ins in src/lib.rs.
    let mut noise = cc::Build::new();
    noise
        .file("c-sv2/sv2_noise.c")
        .include("c-sv2")
        .include("shim")
        .include("cpp-shim")
        .include("../../libsecp256k1/libsecp256k1/include")
        .warnings(false);
    for f in NOISE_FUNCTIONS {
        noise.define(f, Some(format!("ref_{f}").as_str()));
    }
    noise.compile("sv2_noise_reference");

    // Layout assertions of the rust_core headers, on the host.
    cc::Build::new()
        .file("cpp-shim/header_check.c")
        .include("../include")
        .include("cpp-shim")
        .include("shim")
        .compile("header_check");

    // SV2 job construction: the former mining_info_v2.cpp (c-sv2/, with the
    // former create_jobs_task.h).
    // c-mining before shim/: the full mining_utils.h, not the coinbase subset.
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .files(["c-sv2/mining_info_v2.cpp", "cpp-shim/ref_sv2_jobs.cpp"])
        .include("c-mining")
        .include("cpp-shim")
        .include("shim")
        .include("c-sv2")
        .include("main-shim")
        .include("../include")
        .compile("sv2_jobs_reference");

    // ASIC drivers: the former ones (c-asic/, classes and CRC functions
    // renamed to Ref*/ref_*), the driver headers before shim/, whose
    // mining_utils.h is a subset ...
    let mut asic_ref = cc::Build::new();
    asic_ref
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .includes(["c-asic", "c-mining", "cpp-shim", "shim", "../include"])
        .files([
            "c-asic/asic.cpp",
            "c-asic/bm1366.cpp",
            "c-asic/bm1368.cpp",
            "c-asic/bm1370.cpp",
            "c-asic/bm1373.cpp",
            "c-asic/crc.cpp",
            "cpp-shim/asic_wrap.cpp",
        ])
        .define("__bswap32", "__builtin_bswap32")
        .define("PREFIX", "ref_")
        .flag("-include")
        .flag("cpp-shim/host_shim.h")
        .flag("-include")
        .flag("cstdio");
    for (name, renamed) in [
        ("Asic", "RefAsic"),
        ("BM1366", "RefBM1366"),
        ("BM1368", "RefBM1368"),
        ("BM1370", "RefBM1370"),
        ("BM1373", "RefBM1373"),
        ("crc5", "ref_crc5"),
        ("crc16", "ref_crc16"),
        ("crc16_false", "ref_crc16_false"),
        ("crc16_table", "ref_crc16_table"),
    ] {
        asic_ref.define(name, renamed);
    }
    asic_ref.compile("asic_reference");

    // ... and the current Rust driver (asic_driver.h).
    cc::Build::new()
        .cpp(true)
        .std("c++20")
        .warnings(false)
        .includes(["cpp-shim", "shim", "../include"])
        .files(["cpp-shim/asic_wrap_current.cpp"])
        .flag("-include")
        .flag("cpp-shim/host_shim.h")
        .compile("asic_current");

    cpp()
        .file("cpp-shim/serial_stub.cpp")
        .compile("asic_serial_stub");

    // HTTP API authentication: the former OTP/rate limit/network checks.
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .file("c-auth/auth_ref.cpp")
        .include("shim")
        .compile("auth_reference");

    // ArduinoJson's serializer, the reference for ajson::Json::serialize.
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .file("cpp-shim/json_write_ref.cpp")
        .include("arduinojson")
        .compile("json_write_reference");

    // Buck controllers: the former TPS53647/TPS53667 drivers (c-drivers/,
    // verbatim) on a recording I2C stub; once as for most boards and once
    // with Q1373 (no phase shedding on the TPS53667).
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .file("cpp-shim/i2c_stub.cpp")
        .include("c-drivers-shim")
        .include("shim")
        .compile("i2c_stub");
    // the former EMC2302 fan controller driver on the same stub
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .files(["c-drivers/EMC2302.cpp", "cpp-shim/emc_wrap.cpp"])
        .include("c-drivers")
        .include("c-drivers-shim")
        .include("shim")
        .compile("emc2302_reference");
    // the former FXL6408 and TMP451 drivers on the same stub
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .files([
            "c-drivers/fxl6408.cpp",
            "c-drivers/tmp451.cpp",
            "cpp-shim/sensors_wrap.cpp",
        ])
        .include("c-drivers")
        .include("c-drivers-shim")
        .include("shim")
        // the firmware gets these through other headers
        .flag("-include")
        .flag("freertos/task.h")
        .flag("-include")
        .flag("esp_log.h")
        .compile("sensors_reference");
    // the former NerdAxe EMC2101 driver on the same stub
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .files(["c-drivers/nerdaxe/EMC2101.cpp", "cpp-shim/emc2101_wrap.cpp"])
        .include("c-drivers/nerdaxe")
        .include("c-drivers-shim")
        .include("shim")
        .compile("emc2101_reference");
    // the former NerdAxe INA260 and DS4432U drivers on the same stub
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .files([
            "c-drivers/nerdaxe/INA260.cpp",
            "c-drivers/nerdaxe/DS4432U.cpp",
            "cpp-shim/ina_ds_wrap.cpp",
        ])
        .include("c-drivers/nerdaxe")
        .include("c-drivers-shim")
        .include("shim")
        .compile("ina_ds_reference");
    // the former rev7 (NerdQAxe++) TPS546 driver on the same stub
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .files(["c-drivers/rev7/TPS546.cpp", "cpp-shim/tps546_rev7_wrap.cpp"])
        .include("c-drivers/rev7")
        .include("c-drivers-shim")
        .include("shim")
        .flag("-include")
        .flag("freertos/FreeRTOS.h")
        .define("I2C_MASTER_NUM", "((i2c_port_t) 0)")
        .compile("tps546_rev7_reference");
    // the former fan controller and PID library on a recording board stub
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .files([
            "c-fan/fan_controller.cpp",
            "c-fan/pid/PID_v1_bc.cpp",
            "cpp-shim/fan_wrap.cpp",
        ])
        .include("c-fan-shim")
        .include("c-fan")
        .include("shim")
        .compile("fan_reference");
    // the former GET handler documents (c-http-get/, verbatim) on shims
    // returning the differential test's snapshot; small_ref.cpp also as for
    // the NerdAxe/NerdAxeGamma (pidUseMax in /api/v2/settings)
    let get_ref = || {
        let mut b = cc::Build::new();
        b.cpp(true)
            .std("c++17")
            .warnings(false)
            .include("c-http-get")
            .include("../include")
            .include("shim")
            .include("arduinojson");
        for (from, to) in [
            ("Asic", "GetShimAsic"),
            ("Board", "GetShimBoard"),
            ("asic_result_duplicates", "get_shim_asic_result_duplicates"),
            (
                "esp_app_get_description",
                "get_shim_esp_app_get_description",
            ),
            (
                "esp_ota_get_running_partition",
                "get_shim_esp_ota_get_running_partition",
            ),
            (
                "heap_caps_get_free_size",
                "get_shim_heap_caps_get_free_size",
            ),
            ("esp_timer_get_time", "get_shim_esp_timer_get_time"),
            ("get_last_ping_rtt", "get_shim_get_last_ping_rtt"),
            ("get_recent_ping_loss", "get_shim_get_recent_ping_loss"),
            (
                "pool_policy_pool_diff_err",
                "get_shim_pool_policy_pool_diff_err",
            ),
            ("safe_free", "get_shim_safe_free"),
        ] {
            b.define(from, to);
        }
        b
    };
    get_ref()
        .files([
            "c-http-get/system_ref.cpp",
            "c-http-get/v2_ref.cpp",
            "c-http-get/small_ref.cpp",
            "c-http-get/can_ref.cpp",
            "c-http-get/get_shim.cpp",
        ])
        .compile("http_get_reference");
    get_ref()
        .file("c-http-get/small_ref.cpp")
        .define("NERDAXE", None)
        .define("REF_SUFFIX(x)", "x##_nerdaxe")
        .compile("http_get_reference_nerdaxe");
    // the former board logic (c-board/, verbatim) on a minimal Board
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .file("c-board/board_ref.cpp")
        .define("Board", "RefCoreBoard")
        .define("NerdQaxePlus", "RefCoreNerdQaxePlus")
        .compile("board_reference");
    // the former power sequencing (c-board/, verbatim) on recording hardware
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .file("c-board/power_ref.cpp")
        .define("Board", "RefPowerBoard")
        .define("NerdQaxePlus", "RefPowerNerdQaxePlus")
        .define("NerdQaxePlus2", "RefPowerNerdQaxePlus2")
        .define("NerdAxe", "RefPowerNerdAxe")
        .define("NerdaxeGamma", "RefPowerNerdaxeGamma")
        .define("NerdaxeGaia", "RefPowerNerdaxeGaia")
        .compile("power_reference");
    // the former board constructors (profiles) on shim classes
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .file("c-board/profile_ref.cpp")
        .include("../include")
        .define("Board", "RefProfBoard")
        .define("NerdQaxePlus", "RefProfNerdQaxePlus")
        .define("NerdQaxePlus2", "RefProfNerdQaxePlus2")
        .define("NerdAxe", "RefProfNerdAxe")
        .define("NerdaxeGamma", "RefProfNerdaxeGamma")
        .define("NerdaxeGaia", "RefProfNerdaxeGaia")
        .define("NerdOctaxePlus", "RefProfNerdOctaxePlus")
        .define("NerdOctaxeGamma", "RefProfNerdOctaxeGamma")
        .define("NerdQX", "RefProfNerdQX")
        .define("NerdHaxeGamma", "RefProfNerdHaxeGamma")
        .define("NerdEko", "RefProfNerdEko")
        .define("Q1370B", "RefProfQ1370B")
        .define("Q1373B", "RefProfQ1373B")
        .compile("profile_reference");
    // the former WiFi event handler (connect.cpp) on recording shims
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .file("c-wifi/wifi_ref.cpp")
        .compile("wifi_reference");
    // the former app_main() on recording shims
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .file("c-boot/boot_ref.cpp")
        .compile("boot_reference");
    // the former captive portal DNS answers (ESP-IDF example)
    cc::Build::new()
        .file("c-dns/dns_ref.c")
        .warnings(false)
        .compile("dns_reference");
    // the former SV2 authority key decoding (libbase58 from c/)
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .file("c-sv2key/key_ref.cpp")
        .compile("sv2key_reference");
    // the former suffixString()
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .file("c-utils/utils_ref.cpp")
        .compile("utils_reference");
    // the key lists of the former configuration migration
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .file("c-config/keys_ref.cpp")
        .include("main-shim")
        .include("../include")
        .include("shim-sdkconfig")
        .define("NVS_CONFIG_HOST_ACCESSORS", None)
        .flag("-include")
        .flag("shim/sdkconfig_host.h")
        .compile("config_keys_reference");
    // the former NerdAxe TPS546 driver on the same stub
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .files(["c-drivers/nerdaxe/TPS546.cpp", "cpp-shim/tps546_wrap.cpp"])
        .include("c-drivers/nerdaxe")
        .include("c-drivers-shim")
        .include("shim")
        .flag("-include")
        .flag("freertos/FreeRTOS.h")
        .define("I2C_MASTER_NUM", "((i2c_port_t) 0)")
        .compile("tps546_reference");
    for (name, q1373, prefix) in [
        ("tps_reference", false, "ref_tps_"),
        ("tps_reference_q", true, "refq_tps_"),
    ] {
        let mut b = cc::Build::new();
        b.cpp(true)
            .std("c++17")
            .warnings(false)
            .files([
                "c-drivers/TPS53647.cpp",
                "c-drivers/TPS53667.cpp",
                "cpp-shim/tps_wrap.cpp",
            ])
            .include("c-drivers")
            .include("c-drivers-shim")
            .include("cpp-shim")
            .include("shim")
            .define("REFNAME(x)", format!("{prefix}##x").as_str())
            .flag("-include")
            .flag("cmath");
        if q1373 {
            b.define("Q1373", None)
                .define("TPS53647", "RefTPS53647Q")
                .define("TPS53667", "RefTPS53667Q");
        } else {
            b.define("TPS53647", "RefTPS53647")
                .define("TPS53667", "RefTPS53667");
        }
        b.compile(name);
    }

    // HTTP settings bodies: the former ArduinoJson handling (c-http/) and
    // the current apply table, both on the real nvs_config.h with a
    // recording config layer (cfg_stub.cpp). NERDAXE: pidUseMax applies.
    let settings = |files: &[&str], name: &str| {
        cc::Build::new()
            .cpp(true)
            .std("c++17")
            .warnings(false)
            .files(files)
            .include("main-shim")
            .include("arduinojson")
            .include("../include")
            .include("shim-sdkconfig")
            .define("NERDAXE", None)
            .define("NVS_CONFIG_HOST_ACCESSORS", None)
            .flag("-include")
            .flag("shim/sdkconfig_host.h")
            .compile(name);
    };
    settings(
        &[
            "c-http/settings_ref.cpp",
            "c-http/bodies_ref.cpp",
            "c-http/ota_ref.cpp",
            "c-http/apis_ref.cpp",
        ],
        "settings_reference",
    );
    settings(
        &["cpp-shim/bodies_wrap_current.cpp", "cpp-shim/cfg_stub.cpp"],
        "settings_current",
    );

    // Stratum manager decision logic, extracted verbatim by gen_ref_manager.py.
    cpp()
        .file("cpp-shim/ref_manager.cpp")
        .compile("manager_reference");

    cpp()
        .file("cpp-shim/transport_stub.cpp")
        .include("cpp")
        .compile("stratum_transport_stub");

    for dir in [
        "c",
        "c-mining",
        "c-sv2",
        "c-asic",
        "c-auth",
        "c-http",
        "c-http-get",
        "c-board",
        "c-wifi",
        "c-boot",
        "c-dns",
        "c-sv2key",
        "c-utils",
        "c-config",
        "shim-sdkconfig",
        "c-drivers",
        "c-drivers-shim",
        "c-fan",
        "c-fan-shim",
        "arduinojson",
        "main-shim",
        "c-manager",
        "shim",
        "cpp",
        "cpp-shim",
        "../include",
    ] {
        println!("cargo:rerun-if-changed={dir}");
    }
}
