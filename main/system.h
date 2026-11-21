#pragma once

#include <stdint.h>
#include <string.h>

#include "esp_system.h"
#include "esp_wifi.h"

#include "boards/board.h"
#include "network/network_manager.h"
#include "history.h"
#include "system_state.h"

// Configuration and constants
#define STRATUM_USER CONFIG_STRATUM_USER
#define DIFF_STRING_SIZE 12 // Maximum size of the difficulty string
#define MAX_ASIC_JOBS 128   // Maximum number of ASIC jobs allowed

// The system module: state, board error and the system task are in Rust
// (system_state.h); this class keeps the C++ call sites and owns the board
// object.
class System {
  protected:
    Board *m_board = nullptr;
    History m_history;

  public:
    // Task wrapper for FreeRTOS task creation
    static void taskWrapper(void *pvParameters) { system_task(pvParameters); }

    void notifyMiningStarted() {}

    const char *getMacAddress() { return network_mac_addr(); }

    // RSSI (dBm) of the access point, -90 if not connected
    int get_wifi_rssi() { return network_wifi_rssi(); }

    int64_t getStartTime() const { return system_start_us(); }
    double getCurrentHashrate10m() const { return history_current(1); }
    double getCurrentHashrate1m() const { return history_current(0); }
    float getCurrentHashrate() { return system_current_hashrate(); }

    // a hardware fault (the power task): replaces any other error
    void setBoardError(Board::Error error, uint32_t code) { system_set_error((int) error, code); }

    // a non-hardware error (coinbase verification): only if there is none,
    // so it cannot hide (and later clear) a hardware fault
    void setBoardErrorIfNone(Board::Error error, uint32_t code) { system_set_error_if_none((int) error, code); }

    Board::Error getBoardError() const { return (Board::Error) system_error(nullptr); }
    Board::Error getBoardError(uint32_t *code) const { return (Board::Error) system_error(code); }

    // clears the error only if it is `error`
    void clearBoardError(Board::Error error) { system_clear_error((int) error); }

    // WiFi status and IP address, copied (they change at runtime)
    void getWifiStatus(char *buf, size_t len) const { system_wifi_status(buf, len); }
    void getIPAddress(char *buf, size_t len) const { system_ip(buf, len); }
    void setWifiStatus(const char *wifiStatus) { system_set_wifi_status(wifiStatus); }
    void setSsid(const char *ssid) { system_set_ssid(ssid); }

    void setAPState(bool state) { system_set_ap_state(state); }
    bool getAPState() { return system_ap_state(); }

    void setStartupDone() { system_set_startup_done(); }
    bool isStartupDone() { return system_startup_done(); }

    void setBoard(Board *board) { m_board = board; }
    Board *getBoard() { return m_board; }

    History *getHistory() { return &m_history; }

    esp_reset_reason_t showLastResetReason() { return (esp_reset_reason_t) system_read_reset_reason(); }
    const char *getLastResetReason() { return system_reset_reason(); }

    const char *getHostname() { return system_hostname(); }

    void pushShare(int nr) { m_history.pushShare(nr); }
};
