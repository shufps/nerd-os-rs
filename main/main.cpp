// The remaining C++ objects, and the boot steps on them. The boot
// itself and app_main() are Rust (components/rust_core/src/boot.rs,
// boot_firmware.rs); it calls these through boot_cpp_hooks() (boot.h).
#include "esp_log.h"
#include "esp_psram.h"

#include "boards/board.h"
#include "boot.h"
#include "global_state.h"
#include "hashrate_monitor_task.h"
#include "stratum/stratum_manager.h"
#include "system.h"

System SYSTEM_MODULE;

PowerManagementTask POWER_MANAGEMENT_MODULE;
HashrateMonitor HASHRATE_MONITOR;

StratumManager *STRATUM_MANAGER = nullptr;

static const char *TAG = "nerd*axe";

#ifndef CONFIG_SPIRAM
#error "firmware will not work without psram"
#endif

namespace boot_steps {

static Board *s_board = nullptr;

// the firmware is built for one board (the board's define)
static void create_board()
{
#if defined(NERDQAXEPLUS)
    s_board = new Board(BOARD_NERDQAXEPLUS);
#elif defined(NERDQAXEPLUS2)
    s_board = new Board(BOARD_NERDQAXEPLUS2);
#elif defined(NERDOCTAXEPLUS)
    s_board = new Board(BOARD_NERDOCTAXEPLUS);
#elif defined(NERDAXE)
    s_board = new Board(BOARD_NERDAXE);
#elif defined(NERDOCTAXEGAMMA)
    s_board = new Board(BOARD_NERDOCTAXEGAMMA);
#elif defined(NERDAXEGAMMA)
    s_board = new Board(BOARD_NERDAXEGAMMA);
#elif defined(NERDAXEGAIA)
    s_board = new Board(BOARD_NERDAXEGAIA);
#elif defined(NERDHAXEGAMMA)
    s_board = new Board(BOARD_NERDHAXEGAMMA);
#elif defined(NERDEKO)
    s_board = new Board(BOARD_NERDEKO);
#elif defined(NERDQX)
    s_board = new Board(BOARD_NERDQX);
#elif defined(Q1370)
    s_board = new Board(BOARD_Q1370);
#elif defined(Q1373)
    s_board = new Board(BOARD_Q1373);
#else
#error "no board defined"
#endif
}

static void system_set_board()
{
    SYSTEM_MODULE.setBoard(s_board);

    size_t total_psram = esp_psram_get_size();
    ESP_LOGI(TAG, "PSRAM found with %dMB", total_psram / (1024 * 1024));
    ESP_LOGI(TAG, "Found Device Model: %s", s_board->getDeviceModel());
    ESP_LOGI(TAG, "Found Board Version: %d", s_board->getVersion());
}

static void create_stratum_manager(bool dual)
{
    STRATUM_MANAGER = new StratumManager(dual ? StratumManager::PoolMode::DUAL : StratumManager::PoolMode::FAILOVER);
}

} // namespace boot_steps

extern "C" const boot_cpp_hooks_t *boot_cpp_hooks(void)
{
    using namespace boot_steps;
    static const boot_cpp_hooks_t hooks = {
        .board_create = create_board,
        .system_set_board = system_set_board,
        .create_stratum_manager = create_stratum_manager,
    };
    return &hooks;
}
