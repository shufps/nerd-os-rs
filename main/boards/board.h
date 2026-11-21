#pragma once

#include <vector>
#include "nvs_config.h"
#include "../pid/pid_settings.h"
#include "board_core.h"
#include "board_profile.h"
#include "board_rs.h"
#include "driver/gpio.h"

// Ethernet (W5500 interposer) pin map, per board. GPIO_NUM_NC = not wired / not used.
// rst = NC means the board resets the W5500 by RC (no GPIO); irq = NC means polling.
struct EthPins {
    gpio_num_t sclk = GPIO_NUM_NC, mosi = GPIO_NUM_NC, miso = GPIO_NUM_NC,
               cs = GPIO_NUM_NC, rst = GPIO_NUM_NC, irq = GPIO_NUM_NC;
};

// The board: constants, settings and hardware are in Rust (board_rs.h); this
// class keeps the C++ call sites and owns the display theme and the ASIC
// object (main/boards/board.cpp, with the self test).
class Board {
public:
    enum Error {
        NONE,
        TEMP_FAULT,
        VREG_TEMP_FAULT,
        PSU_FAULT,
        IOUT_OC_FAULT,
        VOUT_FAULT,
        COINBASE_VERIFY_FAULT
    };

    static const char* errorToStr(Error err) {
        switch (err) {
            case Error::NONE: return "";
            case Error::TEMP_FAULT: return "MINER OVERHEATED";
            case Error::VREG_TEMP_FAULT: return "VREG OVERHEATED";
            case Error::PSU_FAULT: return "PSU FAULT";
            case Error::IOUT_OC_FAULT: return "CURRENT PROTECTION";
            case Error::VOUT_FAULT: return "VOLTAGE PROTECTION";
            case Error::COINBASE_VERIFY_FAULT: return "VERIFY FAILED";
            default: return "INVALID ERROR";
        }
    }

  protected:
    // copies for the callers that take pointers
    PidSettings m_pidSettings[2] = {};
    EthPins m_ethPins;

    board_info_t info() const
    {
        board_info_t i;
        board_info(&i);
        return i;
    }

  public:
    // creates the board BOARD_* (board_profile.h)
    explicit Board(int board);

    bool initBoard() { return board_init(); }
    bool initAsics() { return board_init_asics(); }

    void loadSettings() { board_load_settings(); }
    const char *getDeviceModel() { return info().device_model; }
    const char *getMiningAgent() { return info().mining_agent; }
    int getVersion() { return info().version; }
    const char *getAsicModel() { return info().asic_model; }
    int getAsicCount() { return info().asic_count; }
    int getAsicJobIntervalMs() { return info().job_interval_ms; }

    bool setAsicFrequency(float f) { return board_set_asic_frequency(f); }
    bool validateFrequency(float frequency) { return board_validate_frequency_logged(frequency); }
    bool validateVoltage(float core_voltage) { return board_validate_voltage_logged(core_voltage); }

    void setVrFrequency(uint32_t freq) { board_set_vr_frequency(freq); }

    bool setVoltage(float core_voltage) { return board_set_voltage(core_voltage); }
    void setFanPolarity(bool invert) { board_set_fan_polarity(invert); }
    void setFanSpeedCh(int channel, float perc) { board_set_fan_speed_ch(channel, perc); }
    void setFanSpeed(float perc) { board_set_fan_speed(perc); }
    void getFanSpeedCh(int channel, uint16_t *rpm) { *rpm = board_fan_rpm(channel); }

    int getNumFans() { return info().num_fans; }

    float getTemperature(int index) { return board_temperature(index); }
    float getVRTemp() { return board_vr_temp(); }
    bool isPIDAvailable() { return true; }

    float getVin() { return board_power_value(0); }
    float getIin() { return board_power_value(1); }
    float getPin() { return board_power_value(2); }
    float getVout() { return board_power_value(3); }
    float getIout() { return board_power_value(4); }
    float getPout() { return board_power_value(5); }

    void requestBuckTelemtry() { board_request_buck_telemetry(); }
    void requestChipTemps() { board_request_chip_temps(); }

    // the chip temperatures are in Rust (board_core.h)
    void setChipTemp(int nr, float temp) { board_chip_temp_set(nr, temp); }
    float getMaxChipTemp() { return board_chip_temp_max(); }
    float getChipTemp(int nr) { return board_chip_temp(nr); }

    void shutdown() { board_shutdown(); }

    Error getFault(uint32_t *status) { return (Error) board_fault(status); }

    bool selfTest();


    uint32_t getAsicMaxDifficulty() { return info().max_difficulty; }
    uint32_t getAsicMinDifficulty() { return info().min_difficulty; }
    uint32_t getAsicMinDifficultyDualPool() { return info().min_difficulty_dual_pool; }

    bool isInitialized() { return board_is_initialized(); }
    bool isBuckInitialized() { return info().buck_initialized; }


    int getAsicVoltageMillis() { return info().asic_voltage_mv; }
    int getAsicFrequency() { return info().asic_frequency; }
    int getAbsMaxAsicFrequency() { return info().abs_max_frequency; }
    int getAbsMinAsicVoltageMillis() { return info().abs_min_voltage_mv; }
    int getAbsMaxAsicVoltageMillis() { return info().abs_max_voltage_mv; }
    int getDefaultAsicVoltageMillis() { return info().default_voltage_mv; }
    int getDefaultAsicFrequency() { return info().default_frequency; }
    int getEcoAsicVoltageMillis() { return info().eco_voltage_mv; }
    int getEcoAsicFrequency() { return info().eco_frequency; }
    uint32_t getDefaultVrFrequency() { return info().default_vr_frequency; }
    uint32_t getVrFrequency() { return info().vr_frequency; }

    float getMinPin() { return info().min_pin; }
    float getMaxPin() { return info().max_pin; }
    float getMinVin() { return info().min_vin; }
    float getMaxVin() { return info().max_vin; }
    // the input current range (A) for the UI gauge
    float getMinCurrentA() const { return info().min_current_a; }
    float getMaxCurrentA() const { return info().max_current_a; }
    float getVrMaxTemp() { return info().vr_max_temp; }

    int getNumTempSensors() { return info().num_temp_sensors; }
    bool isFlipScreenEnabled() { return info().flip_screen; }
    bool isInvertFanPolarityEnabled() { return info().fan_invert; }

    PidSettings *getPidSettings(int ch = 0)
    {
        board_pid_t p = info().pid[ch & 1];
        m_pidSettings[ch & 1] = {p.target_temp, p.p, p.i, p.d};
        return &m_pidSettings[ch & 1];
    }

    int getFanMode(int ch) { return info().fan_mode[ch & 1]; }

    const char *getFanLabel(int ch) const
    {
        board_info_t i = info();
        if (ch < 0 || ch >= i.num_fans) return "";
        return i.fan_labels[ch];
    }

    std::vector<uint32_t> getFrequencyOptions() const
    {
        board_info_t i = info();
        return std::vector<uint32_t>(i.frequencies, i.frequencies + i.frequency_count);
    }

    std::vector<uint32_t> getVoltageOptions() const
    {
        board_info_t i = info();
        return std::vector<uint32_t>(i.voltages, i.voltages + i.voltage_count);
    }

    const char *getSwarmColorName() { return info().swarm_color; }

    bool hasHashrateCounter() { return info().has_hash_counter; }
    bool hasEthernet() { return board_has_ethernet(); }

    // W5500 ethernet pin map of the board's HAT; nullptr = no ethernet HAT wiring
    const EthPins *getEthPins()
    {
        int p[4];
        if (!board_eth_pins(p)) {
            return nullptr;
        }
        m_ethPins.sclk = (gpio_num_t) p[0];
        m_ethPins.mosi = (gpio_num_t) p[1];
        m_ethPins.miso = (gpio_num_t) p[2];
        m_ethPins.cs = (gpio_num_t) p[3];
        return &m_ethPins;
    }

    // called once after initBoard(): boards with a pluggable W5500 HAT probe for it
    void detectEthernet() { board_detect_ethernet(); }

    bool hasCanExtension() { return board_has_can_extension(); }
    bool isCanSlave() { return board_is_can_slave(); }
    // the slave ID (1-based) for CAN telemetry/nonce frames
    uint8_t getCanSlaveId() { return 1; }
    int getCanTxPin() { return board_can_tx_pin(); }
    int getCanRxPin() { return board_can_rx_pin(); }

    const char *getDefaultTheme() { return info().default_theme; }
    bool isShutdown() { return board_is_shutdown(); }
    float getVRTempInt() { return board_vr_temp_internal(); }
};
