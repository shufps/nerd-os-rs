/*
 * The boot (implemented in Rust, components/rust_core/src/boot.rs and
 * boot_firmware.rs, with app_main()): the steps on the remaining C++
 * objects, which main.cpp provides.
 */

#ifndef BOOT_H
#define BOOT_H

#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct
{
    /* new Board(BOARD_*) (the Rust board_create()) */
    void (*board_create)(void);
    /* SYSTEM_MODULE.setBoard() and the board/PSRAM log lines */
    void (*system_set_board)(void);
    /* STRATUM_MANAGER = new StratumManager(dual or failover) */
    void (*create_stratum_manager)(bool dual);
} boot_cpp_hooks_t;

const boot_cpp_hooks_t *boot_cpp_hooks(void);

#ifdef __cplusplus
}
#endif

#endif /* BOOT_H */
