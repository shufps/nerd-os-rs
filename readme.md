[![](https://dcbadge.vercel.app/api/server/3E8ca2dkcC)](https://discord.gg/3E8ca2dkcC)

# NerdOS (Rust)

Firmware for the NerdAxe and NerdQAxe family of Bitcoin miners (ESP32-S3,
ESP-IDF 5.3). The firmware core is written in Rust: Stratum v1 and v2, pool
management, the job pipeline, ASIC and board drivers, power and fan control,
the HTTP API, networking, OTA and boot. The LVGL display and the ASIC self
test stay C/C++. The web UI is AxeOS-based.

This is the Rust line of the
[ESP-Miner-NerdQAxePlus](https://github.com/shufps/ESP-Miner-NerdQAxePlus)
firmware. Both lines share the partition layout and the settings stored on the
device.

## Supported boards

| Device | `BOARD` | ASIC |
| --- | --- | --- |
| NerdAxe | `NERDAXE` | 1× BM1366 |
| NerdAxe Gamma | `NERDAXEGAMMA` | 1× BM1370 |
| NerdAxe Gaia | `NERDAXEGAIA` | 1× BM1373 |
| NerdQAxe+ | `NERDQAXEPLUS` | 4× BM1368 |
| NerdQAxe++ | `NERDQAXEPLUS2` | 4× BM1370 |
| NerdQX | `NERDQX` | 4× BM1370 |
| NerdHaxe-γ | `NERDHAXEGAMMA` | 6× BM1370 |
| NerdOCTAXE+ | `NERDOCTAXEPLUS` | 8× BM1368 |
| NerdOCTAXE-γ | `NERDOCTAXEGAMMA` | 8× BM1370 |
| NerdEKO | `NERDEKO` | 12× BM1370 |
| Q1370 | `Q1370` | 4× BM1370 |
| Q1373 | `Q1373` | 4× BM1373 |

## Features

- Stratum v1 and Stratum v2 (encrypted Noise connection), optional TLS for v1
- Primary and fallback pool, or dual-pool mining with a configurable balance
- Coinbase verification: checks that the pool pays to your address
- Web UI and REST API ([docs/API.md](docs/API.md)), live log over WebSocket
- Firmware updates from the web UI (GitHub releases or file upload)
- Optional OTP (TOTP) protection for settings and updates
- WiFi with a setup access point (captive portal); Ethernet on boards with a
  W5500
- InfluxDB telemetry with a Grafana dashboard ([monitoring/](monitoring/))
- Discord webhook alerts (watchdog, block found)
- Fan control: manual, automatic, or PID towards a target temperature

## Installing and updating

### Web flasher (recommended)

The [web flasher](https://shufps.github.io/nerdqaxe-web-flasher/) flashes the
releases of this repository from the browser over USB. It is also the way to
switch between this firmware and the C/C++ line.

[<img src="https://github.com/user-attachments/assets/4168f23a-bfe7-4536-91e3-7af6df9a203a" style="border:5px solid red;width:200px">](https://shufps.github.io/nerdqaxe-web-flasher/)

### Updates from the web UI

A running device updates itself from the **System** page: either directly from
the [releases](https://github.com/shufps/nerd-os-rs/releases) of this
repository or with an uploaded `esp-miner.bin` / `www.bin`. The GitHub
updater only offers releases of this repository.

### Flashing a release with a preset configuration

Each release has a factory image per board (`esp-miner-factory-<board>-<version>.bin`).
With [bitaxetool](https://pypi.org/project/bitaxetool/) it can be flashed
together with a configuration (WiFi, pools, ...), here via the Docker image:

```bash
git clone --recursive https://github.com/shufps/nerd-os-rs
cd nerd-os-rs
cp config.cvs.example config.cvs   # then edit WiFi, pool, ...

./docker/bitaxetool.sh --config config.cvs \
    --firmware esp-miner-factory-NerdQAxe++-<version>.bin -p /dev/ttyACM0
```

Hold the `boot` button while resetting the device to put it into download mode
if flashing does not start.

## Building

Builds run in Docker only; nothing besides Docker is needed on the host. The
image [`shufps/esp-idf-builder-rs:5.3.6`](https://hub.docker.com/r/shufps/esp-idf-builder-rs)
contains ESP-IDF 5.3.6, the Rust Xtensa toolchain, Node.js for the web UI,
`esptool.py`, `bitaxetool` and `nvs_partition_gen.py`. The scripts in
[`docker/`](docker/) pull it on first use; `./docker/build_docker.sh` builds
it locally from [`docker/Dockerfile`](docker/Dockerfile) instead.

```bash
git clone --recursive https://github.com/shufps/nerd-os-rs
cd nerd-os-rs
# if cloned without --recursive:
git submodule update --init --recursive

export BOARD=NERDQAXEPLUS2      # see "Supported boards"
./docker/idf.sh set-target esp32s3
./docker/idf.sh build
```

The results are in `build/`: `esp-miner.bin` (firmware) and `www.bin` (web
UI). After changing `BOARD`, run `./docker/idf.sh fullclean` and
`set-target` again.

### Flashing a build

```bash
./docker/idf.sh -p /dev/ttyACM0 flash monitor
```

This writes the bootloader, partition table, firmware and web UI; the settings
on the device are kept.

To flash a build together with a configuration, generate the settings
partition from `config.cvs` and merge everything into one image (in the
container shell):

```bash
./docker/idf-shell.sh
# inside the container:
nvs_partition_gen.py generate config.cvs config.bin 0x6000
./merge_bin.sh -c nerdos-factory.bin
esptool.py --chip esp32s3 -p /dev/ttyACM0 -b 460800 write_flash \
    --flash_mode dio --flash_freq 80m --flash_size 16MB 0x0 nerdos-factory.bin
exit
```

`./merge_bin.sh nerdos-factory.bin` (without `-c`) builds a factory image
without settings, `-u` one with only the firmware, web UI and OTA data.

### Tests and checks of the Rust code

```bash
docker run --rm -v "$PWD":/home/builder/project \
    -w /home/builder/project/components/rust_core \
    shufps/esp-idf-builder-rs:5.3.6 \
    bash -c 'cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace --release'
```

The host tests include differential tests against the former C/C++
implementations. Details on the Rust component, its layout and the ESP-IDF
bindings are in [components/rust_core/README.md](components/rust_core/README.md).

### Diagnostic builds with core dumps

Panic core dumps are an opt-in diagnostic feature, disabled in normal builds:
a dump can contain WiFi and pool credentials and other task RAM, and its
download endpoint would expose that on the LAN. `sdkconfig.coredump` enables
`CONFIG_ESP_COREDUMP_ENABLE_TO_FLASH`, and the firmware compiles the download
endpoint in exactly when it is set.

```bash
export SDKCONFIG_DEFAULTS="sdkconfig.defaults;sdkconfig.coredump"
./docker/idf.sh set-target esp32s3
./docker/idf.sh build
```

`unset SDKCONFIG_DEFAULTS` and `set-target` again return to a normal build.
In CI, the **`enable_coredump`** option of the build workflow does the same; it
is only allowed for pre-releases, which then also carry the matching
`esp-miner-<board>.elf`.

The dump goes to the 64K `coredump` partition (ESP-IDF ELF format: task
registers and stacks). On such builds the **System** page offers the last dump
for download (with OTP protection if enabled); the file name contains board,
version and the ELF SHA-256 of the crashed build. Decode it with the ELF of
exactly that build (in `./docker/idf-shell.sh`):

```bash
esp-coredump --chip esp32s3 info_corefile --core-format raw \
    --core coredump_<board>_<version>_<elf-sha256>_<timestamp>.bin esp-miner-<board>.elf
```

Or read it from a connected device: `./docker/idf.sh -p /dev/ttyACM0 coredump-info`
(from the matching build directory). Only share dumps with people you trust.

### Display screenshots (development)

A development build can return the display content as a BMP image, for
comparing screens without looking at the device. It is off in normal builds:
the display can show the OTP enrollment QR code.

```bash
./docker/idf.sh -DRUST_CORE_SCREENSHOT=ON reconfigure
./docker/idf.sh build
curl -o screen.bmp http://<device>/api/debug/screen
# a short press of button 1 or 2, to reach the other screens
curl -X POST "http://<device>/api/debug/press?btn=1"
```

`-DRUST_CORE_SCREENSHOT=OFF reconfigure` turns it off again (the option is
kept in the build directory's CMake cache).

## Repository layout

| Path | Content |
| --- | --- |
| `components/rust_core/` | the Rust firmware core ([README](components/rust_core/README.md)) |
| `main/` | display fonts and images (C arrays), web UI (`main/http_server/axe-os/`) |
| `docs/API.md` | REST API |
| `docker/` | build image and helper scripts |
| `monitoring/` | InfluxDB + Grafana setup |
| `config.cvs.example` | settings template for factory images |

## Grafana monitoring

<img src="https://github.com/user-attachments/assets/3c485428-5e48-4761-9717-bd88579a747d" width="600px">

The firmware can send its statistics to InfluxDB. [monitoring/](monitoring/)
contains a Docker Compose setup with InfluxDB and a Grafana dashboard.

## Credits

- Bitaxe / [ESP-Miner](https://github.com/skot/ESP-Miner) developers at OSMU: @skot, @ben, @jhonny
- NerdAxe developer @BitMaker
- [Bitaxe web flasher](https://github.com/bitaxeorg/bitaxe-web-flasher) by [WantClue](https://github.com/WantClue), on which the NerdQAxe web flasher is based

## License

GPL-3.0, see [LICENSE](LICENSE).
