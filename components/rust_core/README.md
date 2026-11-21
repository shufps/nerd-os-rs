# rust_core

The firmware (everything except the display fonts and images and the web
UI in `main/`). cargo builds this crate as one static library, which the
ESP-IDF build links like any other component; the entry point is
`app_main()` in `src/boot_firmware.rs`. Only two symbols are exported to C:
`app_main` and `ws_log_enqueue` (for `ws_log.c`).

| Area | Modules |
|---|---|
| boot, system task | `boot.rs`, `boot_firmware.rs`, `system.rs`, `tasks.rs` |
| board and hardware | `board*.rs`, `asic/`, `tps546.rs`, `tps536x7.rs`, `emc2101.rs`, `emc2302.rs`, `ina260.rs`, `ds4432u.rs`, `power_mgmt.rs`, `fan_control.rs` |
| mining | `jobs.rs`, `mining*.rs`, `hashrate.rs`, `history.rs` |
| pools | `pool*.rs`, `stratum_v1/`, `stratum_v2/`, `stratum_task.rs`, `coinbase.rs` |
| network, HTTP | `network.rs`, `wifi*.rs`, `w5500.rs`, `http_*.rs`, `ota_*.rs` |
| display | `display*.rs`, `ui.rs`, `lvgl.rs` |
| config, OTP, alerts | `config*.rs`, `cfg.rs`, `otp*.rs`, `discord.rs`, `influx.rs` |
| CAN swarm | `can*.rs` |

Supporting modules:
- `src/cutil.rs` reproduces C behaviour (`hex2bin`, newlib's `strtoul`) and
  allocates memory that C frees.
- `src/alloc.rs` is the global allocator, which prefers PSRAM like the
  firmware's `MALLOC()`.
- `src/crypto.rs`: HMAC/HKDF on the (hardware) SHA-256, ChaCha20-Poly1305
  (RustCrypto), random numbers.
- `src/secp.rs` binds the existing libsecp256k1 component, instead of a
  second copy of the library from a crate.
- `src/log.rs` logs through `esp_log_write`.

On the host, `build.rs` compiles libsecp256k1 and `host/esp_host.c`
(stand-ins for `esp_transport_read/write` and `esp_fill_random` that tests
can script).

All Rust code must go into this crate: a second Rust static library would
bring a second copy of `std` into the firmware.

## Toolchain

Xtensa needs Espressif's Rust fork (toolchain `esp`, selected by
`rust-toolchain.toml`). The Docker image in `docker/` installs it. To install
it on the host:

```sh
cargo install espup --locked   # or download a release binary
espup install --targets esp32s3 --toolchain-version 1.80.0.0
```

`idf.py build` then runs cargo automatically.

## Tests

The tests run on the host with the same toolchain:

```sh
cd components/rust_core
cargo test --release
```

- The modules have unit tests (for example BIP-173/350 address vectors).
- `tests/` holds golden tests: the drivers (ASIC, regulators, sensors, fans),
  the board logic, the configuration, the HTTP documents and bodies, the
  authentication, Stratum v1/v2, Noise, the job construction, the pool
  policies and the coinbase verification run on fixed random inputs, and
  their outputs are compared with `tests/golden/<suite>.txt`. These files
  hold the outputs of the former C/C++ code, recorded by differential tests
  while it still compiled for the host (count, SHA-256 digest and the first
  outputs in full). After an intended change of behaviour, `GOLDEN_BLESS=1
  cargo test --release` rewrites them; their diff belongs to the review
  (see `tests/golden/mod.rs`).

## Build notes

- `posix_stubs.c` provides a `chmod` stub. Without it, the linker takes
  `chmod` from libnosys, whose `dir.o` clashes with the ESP-IDF vfs.
- Panics trap immediately (`panic_immediate_abort`), and the ESP-IDF panic
  handler prints the backtrace. Configuring with
  `-DRUST_CORE_PANIC_MESSAGES=ON` keeps Rust's panic messages but costs about
  60 KB of flash.
- Cost at the time of the port: +6 KB firmware size compared to the C version.
- The ESP-IDF bindings (`src/idf/raw.rs`) are generated with bindgen in
  every firmware build. bindgen reads the headers `layout_check.c` and
  `bindings.h` include, with `layout_check.c`'s flags from the build's
  `compile_commands.json`, so the Rust code sees the real struct layouts for
  this sdkconfig. bindgen needs the Xtensa-aware libclang of espup's
  esp-clang (`LIBCLANG_PATH`, set in the Docker image). A new IDF header goes
  into `bindings.h`, and its component into `PRIV_REQUIRES` in
  `CMakeLists.txt`.
- On the host there are no bindings: the modules keep stubs and simple
  models of the IDF types for the host tests. IDF constants are declared
  with `idf_const!` (`src/idf/mod.rs`): the binding's value in the firmware,
  the written value on the host. `layout_check.c` only guards the
  configuration the Rust code relies on (lwIP stats, TWAI clock source).
- For a clippy run of the firmware code, run `cargo clippy` with the
  firmware target and the environment variables the CMake build sets
  (`RUST_CORE_SDKCONFIG`, `RUST_CORE_COMPILE_COMMANDS`), after an
  `idf.py build`.
