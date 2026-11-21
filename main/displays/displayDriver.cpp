#include <inttypes.h>
#include <stdio.h>

#include "lv_conf.h"

#include "driver/gpio.h"
#include "rom/gpio.h" // gpio_pad_select_gpio()
#include "esp_err.h"
#include "esp_lcd_panel_io.h"
#include "esp_lcd_panel_ops.h"
#include "esp_lcd_panel_vendor.h"
#include "esp_log.h"
#include "esp_netif.h"
#include "esp_timer.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

#include "ui_rs.h"
#include "ui_ipc.h"
#include "ui_helpers.h"
#include "global_state.h"
#include "system.h"
#include "macros.h"
#include "button.h"

#include "nvs_config.h"
#include "displayDriver.h"
#include "display_api.h"

#pragma GCC diagnostic ignored "-Wmissing-field-initializers"

static const char *TAG = "TDisplayS3";

#ifdef NERDQX
#define SPLASH1_TIMEOUT_MS 3000
#define SPLASH2_TIMEOUT_MS 5000
#elif defined(Q1370) || defined(Q1373)
#define SPLASH1_TIMEOUT_MS 3000
#define SPLASH2_TIMEOUT_MS 5000
#else
#define SPLASH1_TIMEOUT_MS 3000
#define SPLASH2_TIMEOUT_MS 3000
#endif

// small helpers
static inline int64_t now_us() { return esp_timer_get_time(); }
static inline int32_t elapsed_ms(int64_t start_us, int64_t now) {
    return static_cast<int32_t>((now - start_us) / 1000);
}

DisplayDriver::DisplayDriver() {
    m_animationsEnabled = false;
    m_lastKeypressTime = 0;
    m_displayIsOn = false;
    m_countdownActive = false;
    m_countdownStartTime = 0;
    m_isActiveOverlay = false;
    m_lvglMutex = PTHREAD_MUTEX_INITIALIZER;
    m_isAutoScreenOffEnabled = false;
    m_shutdownCountdownActive = false;
    m_shutdownStartTime = 0;
    m_shutdownLabel = nullptr;
    m_buttonIgnoreUntil_us = 0;
}

void DisplayDriver::loadSettings() {
    PThreadGuard lock(m_lvglMutex);
    m_isAutoScreenOffEnabled = Config::isAutoScreenOffEnabled();
    m_showFoundBlockEnabled = Config::isShowBlockFoundEnabled();

    // when setting was changed, turn on the display LED
    if (!m_isAutoScreenOffEnabled) {
        displayTurnOn();
    }
}

bool DisplayDriver::notifyLvglFlushReady(esp_lcd_panel_io_handle_t panelIo, esp_lcd_panel_io_event_data_t* edata,
                                   void* userCtx) {
    lv_disp_drv_t* dispDriver = (lv_disp_drv_t*)userCtx;
    lv_disp_flush_ready(dispDriver);
    return false;
}

void DisplayDriver::lvglFlushCallback(lv_disp_drv_t* drv, const lv_area_t* area, lv_color_t* colorMap) {
    esp_lcd_panel_handle_t panelHandle = (esp_lcd_panel_handle_t)drv->user_data;
    int offsetx1 = area->x1;
    int offsetx2 = area->x2;
    int offsety1 = area->y1;
    int offsety2 = area->y2;
    // the development screenshot copy (a no-op in normal builds)
    disp_shadow_flush(offsetx1, offsety1, offsetx2, offsety2, (const uint16_t *) colorMap);
    // Copy buffer content to the display
    esp_lcd_panel_draw_bitmap(panelHandle, offsetx1, offsety1, offsetx2 + 1, offsety2 + 1, colorMap);
}

/************ DISPLAY TURN ON/OFF FUNCTIONS *************/
bool DisplayDriver::displayTurnOff(void) {
    if (!m_displayIsOn) {
        return false;
    }
    gpio_set_level(TDISPLAYS3_PIN_NUM_BK_LIGHT, TDISPLAYS3_LCD_BK_LIGHT_OFF_LEVEL);
    gpio_set_level(TDISPLAYS3_PIN_PWR, false);
    ESP_LOGI(TAG, "Screen off");
    m_displayIsOn = false;
    return true;
}

bool DisplayDriver::displayTurnOn(void) {
    if (m_displayIsOn) {
        return false;
    }
    gpio_set_level(TDISPLAYS3_PIN_PWR, true);
    gpio_set_level(TDISPLAYS3_PIN_NUM_BK_LIGHT, TDISPLAYS3_LCD_BK_LIGHT_ON_LEVEL);
    ESP_LOGI(TAG, "Screen on");
    m_displayIsOn = true;
    return true;
}

/************ AUTO TURN OFF DISPLAY FUNCTIONS *************/
void DisplayDriver::startCountdown(void) {
    m_countdownActive = true;
    m_countdownStartTime = esp_timer_get_time();

    if (m_countdownLabel == NULL) {
        lv_obj_t* currentScreen = lv_scr_act();
        lv_obj_t* blackBox = lv_obj_create(currentScreen);
        lv_obj_set_size(blackBox, 200, 100);
        lv_obj_set_style_bg_color(blackBox, lv_color_black(), LV_PART_MAIN);
        lv_obj_set_style_border_width(blackBox, 0, LV_PART_MAIN);
        lv_obj_align(blackBox, LV_ALIGN_CENTER, 0, 0);

        m_countdownLabel = lv_label_create(blackBox);
        lv_label_set_text(m_countdownLabel, "Turning screen off...");
        lv_obj_set_style_text_color(m_countdownLabel, lv_color_white(), LV_PART_MAIN);
        lv_obj_center(m_countdownLabel);
    }
}

void DisplayDriver::displayHideCountdown(void) {
    if (m_countdownLabel) {
        lv_obj_del(lv_obj_get_parent(m_countdownLabel));
        m_countdownLabel = NULL;
    }
}

void DisplayDriver::checkAutoTurnOffScreen(void) {
    if (!m_displayIsOn)
        return;

    int64_t currentTime = esp_timer_get_time();

    if ((currentTime - m_lastKeypressTime) > 30000000) {  // 30 seconds timeout
        if (!m_countdownActive) {
            startCountdown();
        }

        int64_t elapsedTime = (currentTime - m_countdownStartTime) / 1000000;  // Convert to seconds

        if (elapsedTime > 5) {
            displayHideCountdown();
            displayTurnOff();
            m_countdownActive = false;
        }

    } else {
        if (m_countdownActive) {
            displayHideCountdown();
            m_countdownActive = false;
        }
    }
}

void DisplayDriver::startShutdownCountdown() {
    if (m_shutdownCountdownActive) return;

    m_shutdownCountdownActive = true;
    m_isActiveOverlay = true;
    m_shutdownStartTime = esp_timer_get_time();

    lv_obj_t* scr = lv_scr_act();
    lv_obj_t* box = lv_obj_create(scr);
    lv_obj_set_size(box, 200, 100);
    lv_obj_set_style_bg_color(box, lv_color_black(), LV_PART_MAIN);
    lv_obj_set_style_border_width(box, 0, LV_PART_MAIN);
    lv_obj_align(box, LV_ALIGN_CENTER, 0, 0);

    m_shutdownLabel = lv_label_create(box);
    lv_label_set_text(m_shutdownLabel, "Shutdown in 5");
    lv_obj_set_style_text_color(m_shutdownLabel, lv_color_white(), LV_PART_MAIN);
    lv_obj_center(m_shutdownLabel);
}

void DisplayDriver::updateShutdownCountdown() {
    if (!m_shutdownCountdownActive) return;

    int elapsed = (esp_timer_get_time() - m_shutdownStartTime) / 1000000;
    int remaining = 5 - elapsed;
    if (remaining < 0) remaining = 0;

    if (m_shutdownLabel) {
        char buf[32];
        snprintf(buf, sizeof(buf), "Shutdown in %d", remaining);
        lv_label_set_text(m_shutdownLabel, buf);
    }

    // After 5s → trigger shutdown
    if (elapsed >= 5) {
        hideShutdownCountdown();
        enterState(UiState::PowerOff, esp_timer_get_time());
    }
}

void DisplayDriver::hideShutdownCountdown() {
    if (m_shutdownLabel) {
        lv_obj_del(lv_obj_get_parent(m_shutdownLabel));
        m_shutdownLabel = nullptr;
    }
    m_shutdownCountdownActive = false;
    m_isActiveOverlay = false;
}


void DisplayDriver::increaseLvglTick() {
    lv_tick_inc(TDISPLAYS3_LVGL_TICK_PERIOD_MS);
}

// Refresh screen values
void DisplayDriver::refreshScreen(void) {
    // NOP
}

void DisplayDriver::showError(const char *error_message, uint32_t error_code) {
    PThreadGuard lock(m_lvglMutex);
    // hide the overlay and free the memory in case it was open
    // the overlay is replaced if it was open
    ui_show_error(error_message, error_code);
    m_isActiveOverlay = true;
    refreshScreen();
}

void DisplayDriver::hideError() {
    PThreadGuard lock(m_lvglMutex);
    // hide the overlay and free the memory
    ui_hide_error();
    m_isActiveOverlay = false;
}

void DisplayDriver::showFoundBlockOverlay() {
    PThreadGuard lock(m_lvglMutex);
    // hide the overlay and free the memory in case it was open
    ui_hide_image_overlay();

    // not enabled?
    if (!m_showFoundBlockEnabled) {
        return;
    }

    // now show the (new) image overlay
    ui_show_found_block();
    m_foundBlockOverlay = true;
    refreshScreen();
}

void DisplayDriver::hideFoundBlockOverlay() {
    // called from the LVGL task (button press) while the system task may show a new
    // one for another block
    PThreadGuard lock(m_lvglMutex);
    // hide the overlay and free the memory
    ui_hide_image_overlay();
    m_foundBlockOverlay = false;
}

void DisplayDriver::lvglTimerTaskWrapper(void *param) {
    DisplayDriver *display = (DisplayDriver*) param;
    display->lvglTimerTask(NULL);
}

void DisplayDriver::safe_screen_change(lv_obj_t * new_scr, lv_scr_load_anim_t anim_type, uint32_t speed, uint32_t delay)
{
    m_screenAnimationRunning = true;
    _ui_screen_change(new_scr, anim_type, speed, delay);

}

bool DisplayDriver::enterState(UiState s, int64_t now)
{
    // we already are in this state
    if (m_state == s) {
        return true;
    }
    UiState previousState = m_state;

    m_state = s;
    m_stateStart_us = now;

    switch (m_state) {
    case UiState::NOP:
        // NOP
        break;
    case UiState::Splash1:
        ESP_LOGI(TAG, "enter state splash1");
        enableLvglAnimations(true);
        break;

    case UiState::Splash2:
        ESP_LOGI(TAG, "enter state splash2");
        enableLvglAnimations(true);
        safe_screen_change(ui_screen(UI_SCREEN_SPLASH2), LV_SCR_LOAD_ANIM_FADE_ON, 500, 0);
        ui_drop_splash(UI_SCREEN_SPLASH1);
        break;

    case UiState::Wait:
        ESP_LOGI(TAG, "enter state wait");
        // Keep Splash2 visible — system task decides next screen (Portal or Mining).
        // Cleaning Splash2 here would leave an empty (white) screen with no replacement.
        break;

    case UiState::Portal:
        ESP_LOGI(TAG, "enter state portal");
        ui_drop_splash(UI_SCREEN_SPLASH2);
        enableLvglAnimations(true);
        safe_screen_change(ui_screen(UI_SCREEN_PORTAL), LV_SCR_LOAD_ANIM_FADE_ON, 500, 0);
        break;

    case UiState::Mining:
        ESP_LOGI(TAG, "enter state mining");
        ui_drop_splash(UI_SCREEN_SPLASH2);
        enableLvglAnimations(true);
        if (previousState == UiState::GlobalStats) {
            safe_screen_change(ui_screen(UI_SCREEN_MINING), LV_SCR_LOAD_ANIM_MOVE_LEFT, 350, 0);
        } else {
            safe_screen_change(ui_screen(UI_SCREEN_MINING), LV_SCR_LOAD_ANIM_FADE_ON, 500, 0);
        }
        break;

    case UiState::SettingsScreen:
        ESP_LOGI(TAG, "enter state settings screen");
        enableLvglAnimations(true);
        safe_screen_change(ui_screen(UI_SCREEN_SETTINGS), LV_SCR_LOAD_ANIM_MOVE_LEFT, 350, 0);
        break;

    case UiState::BTCScreen:
        ESP_LOGI(TAG, "enter state btc screen");
        enableLvglAnimations(true);
        safe_screen_change(ui_screen(UI_SCREEN_BTC), LV_SCR_LOAD_ANIM_MOVE_LEFT, 350, 0);
        break;

    case UiState::GlobalStats:
        ESP_LOGI(TAG, "enter state global stats");
        enableLvglAnimations(true);
        safe_screen_change(ui_screen(UI_SCREEN_GLOBAL), LV_SCR_LOAD_ANIM_MOVE_LEFT, 350, 0);
        break;
    case UiState::ShowQR:
        ESP_LOGI(TAG, "enter qr state");
        enableLvglAnimations(true);
        safe_screen_change(ui_screen(UI_SCREEN_QR), LV_SCR_LOAD_ANIM_FADE_ON, 500, 0);
        break;
    case UiState::Identify:
        ESP_LOGI(TAG, "enter state identify (blink %lums)", m_identifyDuration_ms);
        break;
    case UiState::PowerOff:
        ui_power_off_init();
        enableLvglAnimations(false);
        safe_screen_change(ui_screen(UI_SCREEN_POWER_OFF), LV_SCR_LOAD_ANIM_NONE, 0, 0);
        POWER_MANAGEMENT_MODULE.shutdown();
        break;
    }
    return true;
}


void DisplayDriver::updateState(int64_t now, bool btn1Press, bool btn2Press, bool btnBothLongPress)
{
    const int ms = elapsed_ms(m_stateStart_us, now);

    if (btnBothLongPress) {
        enterState(UiState::PowerOff, now);
        return;
    }

    switch (m_state) {
    case UiState::NOP:
        // NOP
        break;
    case UiState::Splash1:
        if (ms >= SPLASH1_TIMEOUT_MS) {
            enterState(UiState::Splash2, now);
        }
        break;

    case UiState::Splash2:
        if (ms >= SPLASH2_TIMEOUT_MS) {
            enterState(UiState::Wait, now);
        }
        break;

    case UiState::Wait:
        // NOP
        break;

    case UiState::Portal:
        enterState(UiState::Portal, now);
        break;

    case UiState::Mining:
        if (ledControl(btn1Press, btn2Press)) {
            break;
        }
        if (btn1Press) {
            apis_enable_fetching();
            enterState(UiState::SettingsScreen, now);
        } else {
            enterState(UiState::Mining, now);
        }
        break;
    case UiState::SettingsScreen:
        if (ledControl(btn1Press, btn2Press)) {
            break;
        }
        if (btn1Press) {
            enterState(UiState::BTCScreen, now);
            apis_enable_fetching();
        }
        break;
    case UiState::BTCScreen:
        if (ledControl(btn1Press, btn2Press)) {
            break;
        }
        if (btn1Press) {
            enterState(UiState::GlobalStats, now);
        }
        break;
    case UiState::GlobalStats:
        if (ledControl(btn1Press, btn2Press)) {
            break;
        }
        if (btn1Press) {
            enterState(UiState::Mining, now);
        }
        break;
    case UiState::ShowQR:
        if (btn1Press || btn2Press) {
            // abort enrollment
            otp_disable_enrollment();
            enterState(UiState::Mining, now);
        }
        break;
    case UiState::Identify:
        if (btn1Press || btn2Press || (uint32_t) ms >= m_identifyDuration_ms) {
            m_isActiveOverlay = false;
            displayTurnOn();
            enterState(UiState::Mining, now);
        } else {
            // blink: 500ms on, 500ms off
            if ((ms / 500) % 2 == 0) {
                displayTurnOn();
            } else {
                displayTurnOff();
            }
        }
        break;
    case UiState::PowerOff:
        // NOP
        break;
    }

}



void DisplayDriver::waitForSplashs() {
    // wait until state is not Splash1 or Splash2
    while ((int) m_state < (int) UiState::Wait) {
        vTaskDelay(pdMS_TO_TICKS(100));
    }
}

bool DisplayDriver::ledControl(bool btn1, bool btn2) {
    // btn1 turns it on
    if (btn1) {
        return displayTurnOn();
    }

    // btn2 toggles the LED
    if (btn2) {
        if (!m_displayIsOn) {
            return displayTurnOn();
        }
        return displayTurnOff();
    }
    return false;
}

uint32_t DisplayDriver::handleLvglTick(int32_t &elapsed_Ani_cycles)
{
    uint32_t wait_ms = 0;

    {
        PThreadGuard lock(m_lvglMutex);
        increaseLvglTick();
        wait_ms = lv_timer_handler();
    }

    if (m_animationsEnabled) {
        const uint32_t fast_cap = 5; // ~200 FPS
        if (++elapsed_Ani_cycles > 80) {
            m_animationsEnabled = false;
            elapsed_Ani_cycles = 0;
        }
        uint32_t sleep_ms = std::min(wait_ms, fast_cap);
        vTaskDelay(pdMS_TO_TICKS(sleep_ms));
        return sleep_ms;
    }

    const uint32_t idle_cap = 50;
    uint32_t sleep_ms = (wait_ms > 0 && wait_ms < idle_cap) ? wait_ms : idle_cap;
    vTaskDelay(pdMS_TO_TICKS(sleep_ms));
    return sleep_ms;
}

void DisplayDriver::processButtons(Button &btn1, Button &btn2, int64_t tnow,
                                   bool &btn1Press, bool &btn2Press, bool &btnBothLongPress)
{
    btn1.update();
    btn2.update();

    uint32_t evt1 = btn1.getEvent();
    uint32_t evt2 = btn2.getEvent();
    // simulated short presses of the development build (always 0 otherwise)
    uint32_t sim = disp_debug_take_press();
    if (sim & 1) evt1 |= BTN_EVENT_SHORTPRESS;
    if (sim & 2) evt2 |= BTN_EVENT_SHORTPRESS;
    bool bothPressed = (evt1 & BTN_EVENT_PRESSED) && (evt2 & BTN_EVENT_PRESSED);
    bool anyPressed = (evt1 & BTN_EVENT_PRESSED) || (evt2 & BTN_EVENT_PRESSED);

    if (anyPressed) {
        m_lastKeypressTime = tnow;
    }

    // Ignore all button events within 200ms of both released
    if (esp_timer_get_time() < m_buttonIgnoreUntil_us) {
        btn1.clearEvent();
        btn2.clearEvent();
        return;
    }

    // Hide found-block overlay if active (own flag: other overlays such as errors
    // come and go independently and must not make it undismissable)
    if ((evt1 & BTN_EVENT_SHORTPRESS || evt2 & BTN_EVENT_SHORTPRESS) && m_foundBlockOverlay) {
        hideFoundBlockOverlay();
        btn1.clearEvent();
        btn2.clearEvent();
        return;
    }

    // --- Shutdown countdown handling ---
    if (bothPressed) {
        if (!m_shutdownCountdownActive) {
            startShutdownCountdown();
        } else {
            updateShutdownCountdown();
        }
        return;
    }

    if (m_shutdownCountdownActive && !bothPressed) {
        hideShutdownCountdown();
        m_buttonIgnoreUntil_us = esp_timer_get_time() + 200 * 1000; // 200ms ignore
        btn1.clearEvent();
        btn2.clearEvent();
        return;
    }

    // Normal button events
    if ((evt1 & BTN_EVENT_LONGPRESS) && (evt2 & BTN_EVENT_LONGPRESS)) {
        btnBothLongPress = true;
        btn1.clearEvent();
        btn2.clearEvent();
    } else {
        if (evt1 & BTN_EVENT_SHORTPRESS) {
            m_lastKeypressTime = tnow;
            btn1Press = true;
            btn1.clearEvent();
        }
        if (evt2 & BTN_EVENT_SHORTPRESS) {
            m_lastKeypressTime = tnow;
            btn2Press = true;
            btn2.clearEvent();
        }
    }
}

void DisplayDriver::handleUiQueueMessages(ui_msg_t &msg, int64_t tnow)
{
    if (xQueueReceive(g_ui_queue, &msg, 0) != pdTRUE) return;

    switch (msg.type) {
        case UI_CMD_SHOW_QR: {
            if (!otp_enrollment_active()) {
                ESP_LOGE(TAG, "no otp enrollment active");
                break;
            }
            int size = 0;
            uint8_t* qrBuf = (uint8_t*) otp_dev_qr(&size);
            if (ui_create_qr(qrBuf, size))
                enterState(UiState::ShowQR, tnow);
            m_isActiveOverlay = true;
            break;
        }
        case UI_CMD_HIDE_QR:
            m_isActiveOverlay = false;
            enterState(UiState::Mining, tnow);
            break;
        case UI_CMD_IDENTIFY:
            m_identifyDuration_ms = msg.param;
            m_isActiveOverlay = true;
            enterState(UiState::Identify, tnow);
            break;
    }

    if (msg.payload) {
        free(msg.payload);
        msg.payload = nullptr;
    }
}

void DisplayDriver::handleAutoOffAndOverlays()
{
    if (m_isActiveOverlay || m_foundBlockOverlay) {
        displayTurnOn();
    } else if (m_isAutoScreenOffEnabled) {
        checkAutoTurnOffScreen();
    }
}

void DisplayDriver::lvglTimerTask(void *param)
{
    displayTurnOn();
    m_lastKeypressTime = now_us();
    enterState(UiState::Splash1, now_us());

    int32_t elapsed_Ani_cycles = 0;
    Button btn1(PIN_BUTTON_1, 5000);
    Button btn2(PIN_BUTTON_2, 5000);
    ui_msg_t msg;

    while (true) {
        const int64_t tnow = now_us();
        uint32_t wait_ms = handleLvglTick(elapsed_Ani_cycles);

        if (POWER_MANAGEMENT_MODULE.isShutdown()) {
            // switch into poweroff state
            enterState(UiState::PowerOff, tnow);
            vTaskDelay(pdMS_TO_TICKS(wait_ms));
            continue;
        }

        // --- Handle buttons ---
        bool btn1Press = false, btn2Press = false, btnBothLongPress = false;
        processButtons(btn1, btn2, tnow, btn1Press, btn2Press, btnBothLongPress);

        // animation is running, ignore buttons
        if (m_screenAnimationRunning) {
            btn1Press = false;
            btn2Press = false;
            btnBothLongPress = false;
        }

        // --- Handle queued UI messages ---
        handleUiQueueMessages(msg, tnow);

        // --- Handle auto turn-off and overlays ---
        handleAutoOffAndOverlays();

        // --- Update FSM state ---
        updateState(tnow, btn1Press, btn2Press, btnBothLongPress);
    }
}


// Función para activar las actualizaciones
void DisplayDriver::enableLvglAnimations(bool enable)
{
    m_animationsEnabled = enable;
}

void DisplayDriver::mainCreatSysteTasks(void)
{
    xTaskCreatePinnedToCore(lvglTimerTaskWrapper, "lvgl Timer", 6000, (void*) this, 4, NULL, 1); // Antes 10000
}

lv_obj_t *DisplayDriver::initTDisplayS3(void)
{
    static lv_disp_draw_buf_t disp_buf; // contains internal graphic buffer(s) called draw buffer(s)
    static lv_disp_drv_t disp_drv;      // contains callback functions
    // GPIO configuration
    ESP_LOGI(TAG, "Turn off LCD backlight");
    gpio_config_t bk_gpio_config = {.pin_bit_mask = 1ULL << TDISPLAYS3_PIN_NUM_BK_LIGHT, .mode = GPIO_MODE_OUTPUT};
    ESP_ERROR_CHECK(gpio_config(&bk_gpio_config));

    gpio_pad_select_gpio(TDISPLAYS3_PIN_NUM_BK_LIGHT);
    gpio_pad_select_gpio(TDISPLAYS3_PIN_RD);
    gpio_pad_select_gpio(TDISPLAYS3_PIN_PWR);
    // esp_rom_gpio_pad_select_gpio(TDISPLAYS3_PIN_NUM_BK_LIGHT);
    // esp_rom_gpio_pad_select_gpio(TDISPLAYS3_PIN_RD);
    // esp_rom_gpio_pad_select_gpio(TDISPLAYS3_PIN_PWR);

    gpio_set_direction(TDISPLAYS3_PIN_NUM_BK_LIGHT, GPIO_MODE_OUTPUT);
    gpio_set_direction(TDISPLAYS3_PIN_RD, GPIO_MODE_OUTPUT);
    gpio_set_direction(TDISPLAYS3_PIN_PWR, GPIO_MODE_OUTPUT);

    gpio_set_level(TDISPLAYS3_PIN_RD, true);
    gpio_set_level(TDISPLAYS3_PIN_NUM_BK_LIGHT, TDISPLAYS3_LCD_BK_LIGHT_OFF_LEVEL);

    ESP_LOGI(TAG, "Initialize Intel 8080 bus");
    esp_lcd_i80_bus_handle_t i80_bus = NULL;
    esp_lcd_i80_bus_config_t bus_config = {.dc_gpio_num = TDISPLAYS3_PIN_NUM_DC,
                                           .wr_gpio_num = TDISPLAYS3_PIN_NUM_PCLK,
                                           .clk_src = LCD_CLK_SRC_DEFAULT,
                                           .data_gpio_nums =
                                               {
                                                   TDISPLAYS3_PIN_NUM_DATA0,
                                                   TDISPLAYS3_PIN_NUM_DATA1,
                                                   TDISPLAYS3_PIN_NUM_DATA2,
                                                   TDISPLAYS3_PIN_NUM_DATA3,
                                                   TDISPLAYS3_PIN_NUM_DATA4,
                                                   TDISPLAYS3_PIN_NUM_DATA5,
                                                   TDISPLAYS3_PIN_NUM_DATA6,
                                                   TDISPLAYS3_PIN_NUM_DATA7,
                                               },
                                           .bus_width = 8,
                                           .max_transfer_bytes = LVGL_LCD_BUF_SIZE * sizeof(uint16_t),
                                           .psram_trans_align = LCD_PSRAM_TRANS_ALIGN,
                                           .sram_trans_align = LCD_SRAM_TRANS_ALIGN};
    ESP_ERROR_CHECK(esp_lcd_new_i80_bus(&bus_config, &i80_bus));
    esp_lcd_panel_io_handle_t io_handle = NULL;
    esp_lcd_panel_io_i80_config_t io_config = {
        .cs_gpio_num = TDISPLAYS3_PIN_NUM_CS,
        .pclk_hz = TDISPLAYS3_LCD_PIXEL_CLOCK_HZ,
        .trans_queue_depth = 20,
        .on_color_trans_done = notifyLvglFlushReady,
        .user_ctx = &disp_drv,
        .lcd_cmd_bits = TDISPLAYS3_LCD_CMD_BITS,
        .lcd_param_bits = TDISPLAYS3_LCD_PARAM_BITS,
        .dc_levels =
            {
                .dc_idle_level = 0,
                .dc_cmd_level = 0,
                .dc_dummy_level = 0,
                .dc_data_level = 1,
            }
    };
    ESP_ERROR_CHECK(esp_lcd_new_panel_io_i80(i80_bus, &io_config, &io_handle));

    ESP_LOGI(TAG, "Install LCD driver of st7789");
    esp_lcd_panel_handle_t panel_handle = NULL;

    esp_lcd_panel_dev_config_t panel_config = {
        .reset_gpio_num = TDISPLAYS3_PIN_NUM_RST,
        .rgb_ele_order = LCD_RGB_ELEMENT_ORDER_RGB,
        .bits_per_pixel = 16,
    };

    ESP_ERROR_CHECK(esp_lcd_new_panel_st7789(io_handle, &panel_config, &panel_handle));

    esp_lcd_panel_reset(panel_handle);
    esp_lcd_panel_init(panel_handle);
    esp_lcd_panel_invert_color(panel_handle, true);

    esp_lcd_panel_swap_xy(panel_handle, true);

    Board *board = SYSTEM_MODULE.getBoard();
    if (!board->isFlipScreenEnabled()) {
        esp_lcd_panel_mirror(panel_handle, true, false);
    } else {
        esp_lcd_panel_mirror(panel_handle, false, true);
    }

    // the gap is LCD panel specific, even panels with the same driver IC, can have different gap value
    esp_lcd_panel_set_gap(panel_handle, 0, 35);

    ESP_ERROR_CHECK(esp_lcd_panel_disp_on_off(panel_handle, true));

    ESP_LOGI(TAG, "Turn on LCD backlight");
    gpio_set_level(TDISPLAYS3_PIN_PWR, true);
    gpio_set_level(TDISPLAYS3_PIN_NUM_BK_LIGHT, TDISPLAYS3_LCD_BK_LIGHT_ON_LEVEL);

    ESP_LOGI(TAG, "Initialize LVGL library");
    lv_init();
    // alloc draw buffers used by LVGL
    // it's recommended to choose the size of the draw buffer(s) to be at least 1/10 screen sized
    lv_color_t *buf1 = (lv_color_t*) MALLOC_DMA(LVGL_LCD_BUF_SIZE * sizeof(lv_color_t));
    assert(buf1);
    // initialize LVGL draw buffers
    lv_disp_draw_buf_init(&disp_buf, buf1, NULL, LVGL_LCD_BUF_SIZE);

    ESP_LOGI(TAG, "Register display driver to LVGL");
    lv_disp_drv_init(&disp_drv);
    disp_drv.hor_res = TDISPLAYS3_LCD_H_RES;
    disp_drv.ver_res = TDISPLAYS3_LCD_V_RES;
    disp_drv.flush_cb = lvglFlushCallback;
    disp_drv.draw_buf = &disp_buf;
    disp_drv.user_data = panel_handle;
    lv_disp_t *disp = lv_disp_drv_register(&disp_drv);

    // Configuration is completed.

    ESP_LOGI(TAG, "Install LVGL tick timer");
    // Tick interface for LVGL (using esp_timer to generate 2ms periodic event)
    /*const esp_timer_create_args_t lvgl_tick_timer_args = {
        .callback = &example_increaseLvglTick,
        .name = "lvgl_tick"
    };*/
    esp_timer_handle_t lvgl_tick_timer = NULL;
    // ESP_ERROR_CHECK(esp_timer_create(&lvgl_tick_timer_args, &lvgl_tick_timer));
    // ESP_ERROR_CHECK(esp_timer_start_periodic(lvgl_tick_timer, TDISPLAYS3_LVGL_TICK_PERIOD_MS * 1000));

    ESP_LOGI(TAG, "Display LVGL animation");
    lv_obj_t *scr = lv_disp_get_scr_act(disp);

    return scr;
}

void DisplayDriver::updateCurrentSettings(int pool)
{
    PThreadGuard lock(m_lvglMutex);
    ui_update_settings(pool);
}

void DisplayDriver::updateGlobalState(int pool)
{
    PThreadGuard lock(m_lvglMutex);
    ui_update(pool, m_state == UiState::BTCScreen, m_state == UiState::GlobalStats);
}

void DisplayDriver::updateIpAddress(const char *ip_address_str)
{
    PThreadGuard lock(m_lvglMutex);
    ui_set_ip(ip_address_str);
}

void DisplayDriver::setNetworkIcon(bool eth_connected)
{
    PThreadGuard lock(m_lvglMutex);
    ui_set_net_icon(eth_connected ? 1 : 0);
}

void DisplayDriver::setCanIcon()
{
    PThreadGuard lock(m_lvglMutex);
    ui_set_net_icon(2);
}

void DisplayDriver::logMessage(const char *message)
{
    PThreadGuard lock(m_lvglMutex);
    ui_log(message);
    enableLvglAnimations(true);
    _ui_screen_change(ui_screen(UI_SCREEN_LOG), LV_SCR_LOAD_ANIM_NONE, 500, 0);
}

void DisplayDriver::miningScreen(void)
{
    PThreadGuard lock(m_lvglMutex);
    enterState(UiState::Mining, now_us());
}


void DisplayDriver::portalScreen(const char *message)
{
    PThreadGuard lock(m_lvglMutex);
    strlcpy(m_portalWifiName, message, sizeof(m_portalWifiName));
    ui_set_ssid(m_portalWifiName);
    enterState(UiState::Portal, now_us());
}

void DisplayDriver::updateWifiStatus(const char *message)
{
    PThreadGuard lock(m_lvglMutex);
    ui_set_connect(message);
    refreshScreen();
}

void DisplayDriver::buttonsInit(void)
{
    gpio_pad_select_gpio(PIN_BUTTON_1);
    gpio_set_direction(PIN_BUTTON_1, GPIO_MODE_INPUT);
    gpio_set_pull_mode(PIN_BUTTON_1, GPIO_PULLUP_ONLY);

    gpio_pad_select_gpio(PIN_BUTTON_2);
    gpio_set_direction(PIN_BUTTON_2, GPIO_MODE_INPUT);
    gpio_set_pull_mode(PIN_BUTTON_2, GPIO_PULLUP_ONLY);
}

/**
 * @brief Program starts from here
 *
 */
void DisplayDriver::init(Board* board)
{
    ESP_LOGI("INFO", "Setting Up TDisplayS3 Screen");

    // Inicializa el GPIO para el botón
    buttonsInit();

    // init the ipc
    ui_ipc_init();

    lv_obj_t *scr = initTDisplayS3();

    ui_init(this);
    // manual_lvgl_update();

    // startUpdateScreenTask(); //Start screen update task
    mainCreatSysteTasks();
}
/**************************  Useful Electronics  ****************END OF FILE***/
