# Not tested on hardware

These parts of the Rust port were not run on hardware. They are covered
only by host tests: unit tests, and golden tests with the outputs of the
former C/C++ code where those exist. Each entry names the PR, what is missing and
what a test would need. When an entry has been tested, delete it.

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

## Power / boards

- **rev7 TPS546 path** (also its detection). Only the NerdAxe path ran on
  hardware (Gamma, Gaia). The rev7 path is covered by differential tests
  only. Needs a NerdQAxe++ rev7 (none available).

## Network (`src/drivers/w5500.rs`, `src/net/network.rs`, PRs N1/N2)

Tested on the Q1373 with the cable plugged in:
- W5500 setup, link up and DHCP over ETH;
- WiFi start (identical driver init) and the station connecting to the configured AP;
- the AP switched off and WiFi stopped once ETH has its IP;
- the default netif, MAC, RSSI and status texts.

Tested with the user on the Q1373 (2026-11-19):
- ETH cable pulled and plugged back (WiFi fallback and back to ETH);
- the access point portal with an empty NVS (AP up, captive DNS, WiFi saved through the portal, station connected after the reboot).

Tested with the user on the Q1370 (2026-11-27), WiFi router restarted: beacon timeout, reconnect attempts every ~3.4 s (1 s backoff), connected again after ~52 s (and once more after the router dropped it again), both pools reconnected after ~80 s, no restart.

Tested with the user on the NerdAxe (2026-11-24), after a factory update from GitHub with `keep_config=false` (NVS erased, AP up): the portal screen, the captive portal opening on a phone, the WiFi scan in the portal, the WiFi saved through the portal.

Still to test:
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

## TPS546 faults on NerdAxeGamma/Gaia

A latched TPS546 fault (over-current etc.) sets the board error, keeps the ASIC voltage off and is logged; the home page shows the hardware protection errors. Tested on a NerdAxe Gaia: over-current at 970 mV / 440 MHz (34–35 A, limit 33 A) gave `boardError: 4` and `CURRENT PROTECTION` on the display.

Still to test:
- the home page alert in a browser (standard and Gaia layout);
- a NerdAxe Gamma.

## Found block on the home page

When a block is found, the firmware keeps its height, time, pool and difficulty (`lastBlock` in `GET /api/v2/dashboard`), and the home page shows a golden banner with confetti and a 🎉 in the tab title until it is dismissed (per browser). Tested on the Q1373 with the test stratum server (a replayed block); the user checked the banner, the tab title and the dismiss button in the standard layout.

The user checked the banner in all themes, the Gaia layout included.

Still to test: a real block with its height and the mempool.space link (the test server's coinbase is not decoded, so its height is 0).

## Mining pause and resume

`POST /api/system/pause` and `/resume` (`mining/pause.rs`, the steps in `power/task.rs`). Tested by Claude over the API with serial logs (2026-11-27):
- Q1373 (TPS53667): 20 cycles;
- NerdAxe Gaia (TPS546 + LDO): 10 cycles;
- NerdAxe (DAC): 10 cycles;
- NerdQAxe++ (TPS53647, BM1370): 10 cycles;
- NerdAxe Gamma (TPS546 without LDO): 10 cycles.

In every cycle: paused within 3–8 s (Q1373 and NerdQAxe++ 0.1 W, Gaia and Gamma 5 W, NerdAxe 1.1 W, core voltage 0), all chips found again on resume (~8–13 s), the configured voltage and frequency applied, shares accepted right after, the hashrate as before, no board error, no restart. The user checked the web UI banner and the "MINING PAUSED" box on the display.

Pool connections (2026-11-28, Q1373 in dual pool mode with ckpool SV1 and blitzpool SV2): a pause disconnects both pools (SV2 at once, SV1 within ~20 s), without a failover, a pool error or reconnect attempts while paused; resume reconnects both while the chain is initialized, shares are accepted right after.

Resume after a latched regulator fault, on the NerdAxe Gaia: 970 mV / 440 MHz tripped the TPS546 over-current protection after ~90 s (`boardError` 4, core voltage 0). The settings were set back to 940 mV / 300 MHz without a restart, then pause and resume: the fault and the board error were cleared, the chip was found, mining went on at 940 mV / 300 MHz (~2.0 TH/s, no rejects, no restart).

Still to test:
- the NerdQAxe+ (BM1368 chain) and the rev7 path;
- pause on a CAN master with active slaves.

## Frequency ramp in its own task

The ASIC frequency ramp (`mining/asic/ramp.rs`) runs in its own task with only the UART; the chain's init no longer ramps, and the power task only sets the target, so the fan control and the overheat check keep running during a ramp. A lower core voltage waits until the frequency has come down. Tested by Claude on the Q1373 (2026-11-28): boot and two pause/resume cycles (the init holds the locks ~1–3 s instead of ~8–13 s, the fans followed the rising load during the ramp, the slowest API answer during a resume was 3.1 s), lowering 1010 mV / 425 MHz to 1000 mV / 400 MHz (the voltage after the ramp down) and back (the voltage before the ramp up).

Still to test: the other chips (BM1366 NerdAxe, BM1368 NerdQAxe+, BM1370 NerdQAxe++/Gamma, BM1373 Gaia), and a CAN slave (its frequency comes from the master).
