// The C++ side of the board (board.h): the constructor; everything else is
// in Rust (board_rs.h).
#include <stdlib.h>

#include "board.h"
#include "esp_log.h"

static const char *TAG = "board";

Board::Board(int board)
{
    if (board_info_sizeof() != sizeof(board_info_t) || !board_create(board)) {
        ESP_LOGE(TAG, "no board %d", board);
        abort();
    }
}
