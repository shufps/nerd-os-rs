# Rust parts of the firmware (see README.md). cargo builds the crate as a
# static library, which is linked like any other component library.
# Requires the "esp" Rust toolchain (espup), see README.md.

# tcp_transport: sv2_noise.h includes esp_transport.h; driver/esp_adc: the
# headers layout_check.c checks the Rust mirrors of the IDF structs against
idf_component_register(SRCS "posix_stubs.c" "layout_check.c" "ws_log.c" INCLUDE_DIRS "include"
    REQUIRES tcp_transport PRIV_REQUIRES driver esp_adc esp_wifi mbedtls lwip esp_http_client esp_app_format esp_http_server esp_netif esp_eth esp_partition app_update spiffs nvs_flash espressif__mdns lvgl esp_lcd)

if(IDF_TARGET STREQUAL "esp32s3")
    set(RUST_TARGET "xtensa-esp32s3-espidf")
else()
    message(FATAL_ERROR "rust_core: no Rust target configured for IDF_TARGET ${IDF_TARGET}")
endif()

find_program(CARGO cargo HINTS "$ENV{CARGO_HOME}/bin" "$ENV{HOME}/.cargo/bin" REQUIRED)

set(CARGO_TARGET_DIR "${CMAKE_CURRENT_BINARY_DIR}/target")
set(RUST_LIB "${CARGO_TARGET_DIR}/${RUST_TARGET}/release/librust_core.a")

# A Rust panic normally traps right away; the ESP-IDF panic handler then prints
# PC and backtrace, which idf.py monitor resolves to the Rust source line.
# -DRUST_CORE_PANIC_MESSAGES=ON also prints Rust's panic message, but costs
# about 60 KB of flash for std's formatting and backtrace code.
option(RUST_CORE_PANIC_MESSAGES "Keep Rust panic messages (~60 KB flash)" OFF)
if(RUST_CORE_PANIC_MESSAGES)
    set(BUILD_STD_FEATURES "")
else()
    set(BUILD_STD_FEATURES "panic_immediate_abort")
endif()

# GET /api/debug/screen (the display as BMP), for development only
option(RUST_CORE_SCREENSHOT "Display screenshot route (development only)" OFF)
if(RUST_CORE_SCREENSHOT)
    set(RUST_FEATURES "--features" "screenshot")
else()
    set(RUST_FEATURES "")
endif()

# the Kconfig values for the Rust constants (build.rs, src/sdkconfig.rs)
idf_build_get_property(RUST_CORE_SDKCONFIG SDKCONFIG)

# cargo tracks its own dependencies, so run it on every build and let it decide.
add_custom_target(rust_core_cargo
    COMMAND ${CMAKE_COMMAND} -E env CARGO_TARGET_DIR=${CARGO_TARGET_DIR} RUST_CORE_SDKCONFIG=${RUST_CORE_SDKCONFIG}
            RUST_CORE_COMPILE_COMMANDS=${CMAKE_BINARY_DIR}/compile_commands.json
            ${CARGO} build --release --locked --target ${RUST_TARGET} ${RUST_FEATURES}
            -Zbuild-std=std,panic_abort -Zbuild-std-features=${BUILD_STD_FEATURES}
    WORKING_DIRECTORY ${COMPONENT_DIR}
    BYPRODUCTS ${RUST_LIB}
    USES_TERMINAL
    COMMENT "Building rust_core with cargo")

# Rust std on ESP-IDF calls into newlib, pthread and vfs (opendir, ...), and
# rust_core into mbedtls (SHA-256), libsecp256k1, tcp_transport, log and
# esp_hw_support (esp_fill_random), and the mDNS component (mdns.rs); listing
# them makes them link after it.
add_prebuilt_library(rust_core_lib ${RUST_LIB}
    REQUIRES newlib pthread vfs mbedtls libsecp256k1 tcp_transport log esp_hw_support espressif__mdns esp_driver_gpio esp_http_server app_update spiffs esp_eth esp_driver_spi)
add_dependencies(${COMPONENT_LIB} rust_core_cargo)
target_link_libraries(${COMPONENT_LIB} INTERFACE rust_core_lib)

# See posix_stubs.c; -u makes the linker take our definition.
target_link_libraries(${COMPONENT_LIB} INTERFACE "-u chmod")
# the firmware entry point is in Rust (src/boot_firmware.rs); it takes the
# C++ boot steps from main.cpp (boot_cpp_hooks), which nothing else references
target_link_libraries(${COMPONENT_LIB} INTERFACE "-u app_main" "-u boot_cpp_hooks")
# the websocket log hook (ws_log.c), referenced only by the Rust library
target_link_libraries(${COMPONENT_LIB} INTERFACE "-u ws_log_install" "-u ws_log_uninstall")
# the display bridge (main/displays/display_api.cpp), referenced only by the
# Rust system task
target_link_libraries(${COMPONENT_LIB} INTERFACE "-u disp_update")
