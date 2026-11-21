// The C++ side of the board (board.h): the self test;
// everything else is in Rust (board_rs.h).
#include <stdio.h>

#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

#include "board.h"
#include "esp_log.h"
#include "nvs_config.h"
#include "../displays/displayDriver.h"

static const char *TAG = "board";

Board::Board(int board)
{
    if (board_info_sizeof() != sizeof(board_info_t) || !board_create(board)) {
        ESP_LOGE(TAG, "no board %d", board);
        abort();
    }
}

// Powers the ASICs and shows on the display whether all were found (checks
// the soldering, not the hashrate).
bool Board::selfTest()
{
    // the NerdAxe family's core voltage is lower
    bool axe = board_is_axe();
    const float vmin = axe ? 1.0f : 1.1f;
    const float vmax = axe ? 1.35f : 1.4f;

    char logString[300];

    // Initialize the display
    DisplayDriver *temp_display;
    temp_display = new DisplayDriver();
    temp_display->init(this);

    snprintf(logString, sizeof(logString),
             "\nSelfTest initiated, wait...\r\n"
             "Testing chip: %s\n\n\n\n\n"
             "[Warning] Only checks soldering\nHashrate not checked",
             getAsicModel());
    temp_display->logMessage(logString);

    //Init Asics
    initAsics();
    float power = getPin();
    float Vout = getVout();
    board_info_t i = info();
    bool powerOK = (power > i.min_pin) && (power < i.max_pin);
    bool VrOK = (Vout > vmin) && (Vout < vmax);
    bool allAsicsDetected = (i.chips_detected == i.asic_count);

    snprintf(logString, sizeof(logString),  "\nTest result [%s]:\r\n"
                                            "- Asics detected [%d/%d]\n"
                                            "- Power status: %s (%.2f W)\n"
                                            "- Asic voltage: %s (%.2f V)\r\n\n"
                                            "%s",
                                            i.asic_model,
                                            i.chips_detected, i.asic_count,
                                            powerOK ? "OK" : "Warning", power,
                                            VrOK ? "OK" : "Warning", Vout,
                                            (allAsicsDetected) ? "OOOOOOOO TEST OK!!! OOOOOOO" : "XXXXXXXXX TEST KO XXXXXXXXX");
    temp_display->logMessage(logString);

    //Update SelfTest flag
    if (allAsicsDetected) {
        Config::setSelfTest(false);
        Config::flush();
    }

    // the NerdQAxe family returned true in any case
    return axe ? allAsicsDetected : true;
}
