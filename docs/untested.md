# Not tested on hardware

These parts of the Rust port were not run on hardware. They are covered
only by host tests: unit tests, and golden tests with the outputs of the
former C/C++ code where those exist. Each entry names the PR, what is missing and
what a test would need. When an entry has been tested, delete it.

## Display / system task

- **Error overlay hidden again** (`src/system.rs`). Showing it ran on a
  NerdQAxe++ (coinbase verification fault); that it goes away when the
  error clears was not watched on the screen.

## Boards

- **Boards other than the Q1373, Q1370, NerdAxeGaia, NerdAxe, NerdQAxe+ and NerdQAxe++** (`src/board/mod.rs`).
  All twelve boards build, and the init sequence on the Q1373 and the Gaia
  is the same, line for line, as before. The NerdAxe (DS4432U DAC with
  INA260) and the NerdQAxe++ run the alpha releases. Not run on hardware:
  - NerdOCTAXE+: control pins on GPIOs, chip temperatures from the ASICs
    every 15 s (the same code ran on a NerdQAxe+: hashrate, power and chip
    temperatures as with the C firmware).
  - NerdEKO and NerdHAXE-γ.
  - NerdOCTAXE-γ: VR detection on GPIO3, two TMP451 muxes.
  - NerdAxeGamma.

  Each needs a flash and a comparison with the previous firmware (log of
  the init sequence, `/api/system/info`).
- **CAN extension detection and slave pin on the NerdQAxe+ family**.
  Needs the CAN HAT.

## Power / boards

- **rev7 TPS546 path** (also its detection). Only the NerdAxe path ran on
  hardware (Gamma, Gaia). The rev7 path is covered by differential tests
  only. Needs a NerdQAxe++ rev7 (none available).

## CAN (`src/can/mod.rs`, `src/http/v2/can.rs`)

Tested on the Q1373 as a CAN master without other nodes:
- the TWAI driver starts;
- MASTER_BOOT is queued;
- the status lines appear;
- the settings commands are queued;
- the swarm API (nodes JSON, actions, PATCH, DELETE incl. errors) matches the C++ code.

Tested with the user (2026-11-27) with a Q1370 as master and two Q1373 as slaves:
- slave negotiation (HELLO/ASSIGN, IDs 1 and 2) and the registry in NVS (both slaves known again after a master restart);
- jobs from the master, the nonce filter on the slaves, their shares submitted by the master, telemetry and config in the swarm API;
- identify (display blinks), the display settings and restart sent to a slave;
- "CAN ID: n" on the slaves' displays;
- bus-off recovery: slaves marked offline when the cable was pulled, back and mining after it was plugged in again.

The voltage command (1010 → 1020 → 1010 mV, the TPS53647 followed) and the fan command (manual 60 % and back to PID) were tested on a slave too.

Still to test: the frequency command applied on a slave.

## Network (`src/drivers/w5500.rs`, `src/net/network.rs`, PRs N1/N2)

Tested on the Q1373 with the cable plugged in:
- W5500 setup, link up and DHCP over ETH;
- WiFi start (identical driver init) and the station connecting to the configured AP;
- the AP switched off and WiFi stopped once ETH has its IP;
- the default netif, MAC, RSSI and status texts.

Tested with the user on the Q1373 (2026-11-19):
- ETH cable pulled and plugged back (WiFi fallback and back to ETH);
- the access point portal with an empty NVS (AP up, captive DNS, WiFi saved through the portal, station connected after the reboot).

Tested with the user on the NerdAxe (2026-11-24), after a factory update from GitHub with `keep_config=false` (NVS erased, AP up): the portal screen, the captive portal opening on a phone, the WiFi scan in the portal, the WiFi saved through the portal.

Still to test:
- the WiFi reconnect backoff after a lost AP;
- an ETH init failure (cleanup path). Since the soundness fixes it tears down
  in the IDF order (glue, netif, driver, PHY/MAC) and leaks MAC/PHY instead of
  freeing them when the driver refuses to uninstall. Needs a W5500 that fails
  after the driver install, for example a missing interrupt line.

## Display screens in Rust

The screens of the display (formerly `main/displays/ui.cpp`) are built in Rust (`src/display/ui.rs` on the LVGL layer `src/display/lvgl.rs`); the C++ driver keeps the panel, the LVGL task, the buttons and the screen state machine.

Tested on a Q1373 (screenshots compared with the C++ screens): splash with donation QR, mining, settings, BTC, global stats, the screen cycle with button 1, the network icon switching from WiFi to Ethernet.

Tested later: the power-off screen (Q1373, shutdown over the API), and on the NerdAxe the portal screen and the self test screens.

Still to test:
- the boards with their own theme (every board except Q1370/Q1373, NerdQAxe++ and NerdQX): the backgrounds and the label positions.

## Display driver in Rust

The display driver (formerly `main/displays/displayDriver.cpp`, buttons, UI messages) is in Rust (`src/display/driver.rs`, `src/display/mod.rs`, `src/display/button.rs`), and so is the self test (`board/boot_firmware.rs`).

Tested on a Q1373: panel init, splash sequence, screen cycle with button 1, OTP QR screen with cancel by button, auto screen off (countdown, off, on by button), the real buttons (short presses, button 2 toggling the screen) and the shutdown countdown with both held; the found-block image and its close by a button (a real block replayed by a test stratum server).

Tested with the user on the NerdAxe: the rotated panel (`flip_screen`; the NerdAxe looks normal, the other boards rotate by 180°), and the self test (`selftest` set, best difficulty 0): ASICs found, the result on the display, the flag cleared.

## TPS546 faults on NerdAxeGamma/Gaia

A latched TPS546 fault (over-current etc.) sets the board error, keeps the ASIC voltage off and is logged; the home page shows the hardware protection errors. Tested on a NerdAxe Gaia: over-current at 970 mV / 440 MHz (34–35 A, limit 33 A) gave `boardError: 4` and `CURRENT PROTECTION` on the display.

Still to test:
- the home page alert in a browser (standard and Gaia layout);
- a NerdAxe Gamma.

## Found block on the home page

When a block is found, the firmware keeps its height, time, pool and difficulty (`lastBlock` in `GET /api/v2/dashboard`), and the home page shows a golden banner with confetti and a 🎉 in the tab title until it is dismissed (per browser). Tested on the Q1373 with the test stratum server (a replayed block); the user checked the banner, the tab title and the dismiss button in the standard layout.

The user checked the banner in all themes, the Gaia layout included.

Still to test: a real block with its height and the mempool.space link (the test server's coinbase is not decoded, so its height is 0).
