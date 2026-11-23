# Not tested on hardware

These parts of the Rust port were not run on hardware. They are covered
only by host tests: unit tests, and golden tests with the outputs of the
former C/C++ code where those exist. Each entry names the PR, what is missing and
what a test would need. When an entry has been tested, delete it.

## Display / system task

- **Error overlay hidden again** (`src/system.rs`). Showing it ran on a
  NerdQAxe++ (coinbase verification fault); that it goes away when the
  error clears was not watched on the screen.
- **CAN slave label**. The display shows "CAN ID: n" or "CAN: ..."
  instead of the IP address. Needs a board in CAN slave mode.
- **Boards without a hashrate counter**. The current hashrate comes
  from the 10 min share average on these boards. Only the Q1373, which has
  a counter, was tested after S4.

## Boards

- **Boards other than the Q1373 and the NerdAxeGaia** (`src/board.rs`).
  All twelve boards build, and the init sequence on the Q1373 and the Gaia
  is the same, line for line, as before. Not run on hardware:
  - NerdQAxe+ and NerdOCTAXE+: control pins on GPIOs, chip temperatures
    from the ASICs every 15 s.
  - NerdQAxe++: runs v2.0.0-alpha.1 (flashed with the web flasher); the
    rev7 TPS546 detection is not covered.
  - NerdEKO and NerdHAXE-γ.
  - NerdOCTAXE-γ: VR detection on GPIO3, two TMP451 muxes.
  - Q1370: TMP451 mux on the FXL6408.
  - NerdAxe: DS4432U DAC with INA260.
  - NerdAxeGamma.

  Each needs a flash and a comparison with the previous firmware (log of
  the init sequence, `/api/system/info`).
- **Ethernet HAT detection on the NerdQAxe family**: the W5500 probe
  on the HAT pins, skipped when the CAN extension or a TMP451 mux uses
  them. Needs a NerdQAxe with an ethernet HAT.
- **CAN extension detection and slave pin on the NerdQAxe+ family**.
  Needs the CAN HAT.

## CAN

- **CAN master receive** and **CAN slave protocol**.
  Only differential tests ran. Needs a CAN setup with a
  master and at least one slave board.

## Power / boards

- **rev7 TPS546 path**. Only the NerdAxe path ran on
  hardware (Gamma). The rev7 path is covered by differential tests only.
  Needs a NerdQAxe++ rev7.

## OTA

- **Factory OTA with `keep_config=false`** (`src/ota_factory_task.rs`):
  the NVS erase after the update. Never run on the test boards (it wipes
  the configuration). The update itself with `keep_config=true` ran on the
  Q1373 (GitHub release v1.1.2).

## Network

- **Captive portal DNS from an access point client** (`src/dns.rs`).
  This was tested with `dig` over the LAN, where the server also answers
  with 192.168.4.1. Needs a phone or laptop on the device's access point
  (no WiFi configured).

## CAN (`src/can.rs`, `src/http_can.rs`)

Tested on the Q1373 as a CAN master without other nodes:
- the TWAI driver starts;
- MASTER_BOOT is queued;
- the status lines appear;
- the settings commands are queued;
- the swarm API (nodes JSON, actions, PATCH, DELETE incl. errors) matches the C++ code.

Untested (needs a second board on the bus):
- slave negotiation (HELLO/ASSIGN, registry in NVS);
- slave jobs, nonces, telemetry and config;
- the settings commands applied on a slave;
- a bus-off recovery.

## Network (`src/w5500.rs`, `src/network.rs`, PRs N1/N2)

Tested on the Q1373 with the cable plugged in:
- W5500 setup, link up and DHCP over ETH;
- WiFi start (identical driver init) and the station connecting to the configured AP;
- the AP switched off and WiFi stopped once ETH has its IP;
- the default netif, MAC, RSSI and status texts.

Tested with the user on the Q1373 (2026-11-19):
- ETH cable pulled and plugged back (WiFi fallback and back to ETH);
- the access point portal with an empty NVS (AP up, captive DNS, WiFi saved through the portal, station connected after the reboot).

Still to test:
- the WiFi scan in the portal (`/api/v2/wifi/scan` with the AP active);
- the WiFi reconnect backoff after a lost AP;
- an ETH init failure (cleanup path). Since the soundness fixes it tears down
  in the IDF order (glue, netif, driver, PHY/MAC) and leaks MAC/PHY instead of
  freeing them when the driver refuses to uninstall. Needs a W5500 that fails
  after the driver install, for example a missing interrupt line.

## GitHub updater on the Rust repository (`webflasher-rs`)

The web UI's updater lists the releases of `shufps/nerd-os-rs`, and the factory update only accepts URLs under that repository's release downloads (`http_ota.rs`, host-tested); the CI uploads to `webflasher-rs/` in the R2 bucket. Switching between the Rust and the C firmware is meant to go through the web flasher only.

Tested with the first release (v2.0.0-alpha.1): the CI upload into `webflasher-rs/`, and the web flasher installing it on a NerdQAxe++ and the Q1373.

Tested with v2.0.0-alpha.4 on a NerdQAxe++: the updater listing the releases (pre-releases shown with the checkbox) and the factory update from one of them with "Keep Settings".

## Settings profiles: the profile "default"

On the first access to the profiles of a device that never had profiles (no `profiles` blob in NVS: a new device or one updated from v2.0.0-alpha.1 or the C firmware), the profile "default" is created from the current settings and marked active.

Tested on hardware: a NerdQAxe++ updated over OTA from v2.0.0-alpha.1 got "default" (its 600 MHz / 1150 mV, failover without a balance, both fans) marked active; a Q1373 whose list exists kept it unchanged.

## From upstream v1.1.3-alpha.1 (Gaia)

Ported from the C firmware: the Gaia-Dark Nebular theme (#765) and the VR temperature gauge of the Gaia home page scaled to the VR shutdown temperature (#766, `vrOverheatTemp` in `GET /api/v2/dashboard`). The new dashboard field is checked by the differential test and on a Q1373 (two fan channels: `vrOverheatTemp` = fan channel 1's shutdown temperature).

Still to test on a NerdAxe Gaia: the Gaia-Dark theme and the VR gauge on the home page.

## Display screens in Rust

The screens of the display (formerly `main/displays/ui.cpp`) are built in Rust (`src/ui.rs` on the LVGL layer `src/lvgl.rs`); the C++ driver keeps the panel, the LVGL task, the buttons and the screen state machine.

Tested on a Q1373 (screenshots compared with the C++ screens): splash with donation QR, mining, settings, BTC, global stats, the screen cycle with button 1, the network icon switching from WiFi to Ethernet.

Still to test:
- the boards with their own theme (every board except Q1370/Q1373, NerdQAxe++ and NerdQX): the backgrounds and the label positions;
- the portal screen (AP mode with SSID), the "found block" overlay, the power-off screen, the OTP enrollment QR code, the self test screen.

## Display driver in Rust

The display driver (formerly `main/displays/displayDriver.cpp`, buttons, UI messages) is in Rust (`src/display_driver.rs`, `src/display.rs`, `src/display_button.rs`), and so is the self test (`boot_firmware.rs`).

Tested on a Q1373: panel init, splash sequence, screen cycle with button 1, OTP QR screen with cancel by button, auto screen off (countdown, off, on by button), the real buttons (short presses, button 2 toggling the screen) and the shutdown countdown with both held; the found-block image and its close by a button (a real block replayed by a test stratum server).

Still to test:
- identify (CAN command);
- the self test (`selftest` set, best difficulty 0) with its two log screens;
- a board with `flip_screen` (mirrored panel).

## IPv6 (pool connections and web access)

Ported from #18/#19. Tested on a Q1373 over Ethernet with SLAAC (global prefix from the router): IPv6 to a dual-stack TLS pool (solo.atlaspool.io:4333) with accepted shares; fallback to IPv4 when the pool refuses IPv6 (ckpool); web access from a link-local client allowed, from a global IPv6 client refused.

Tested with v2.0.0-alpha.4 on a NerdQAxe++ over Wi-Fi with SLAAC (unique-local prefix): IPv6 to a pool given as an IPv6 literal without and with brackets, and as a dual-stack name, each with accepted shares; web access from a unique-local client allowed. The web UI accepts an IPv6 literal (with or without brackets) as the pool address and still refuses a port or `stratum+tcp://` in it.
