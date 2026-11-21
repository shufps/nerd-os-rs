// The display as C functions for the Rust system task (display_api.h).
#include "display_api.h"

#include "global_state.h"

static DisplayDriver *display() { return SYSTEM_MODULE.getDisplay(); }

extern "C" {
// a screen load animation of the Rust UI ended (ui_rs.h)
void disp_screen_loaded(void *driver) { static_cast<DisplayDriver *>(driver)->setScreenAnimationRunning(false); }
void disp_load_settings(void) { SYSTEM_MODULE.loadSettings(); }
void disp_wait_for_splashs(void) { display()->waitForSplashs(); }
void disp_portal_screen(const char *ssid) { display()->portalScreen(ssid); }
void disp_update_ip(const char *ip) { display()->updateIpAddress(ip); }
void disp_set_network_icon(bool eth) { display()->setNetworkIcon(eth); }
void disp_set_can_icon(void) { display()->setCanIcon(); }
void disp_mining_screen(void) { display()->miningScreen(); }
void disp_show_found_block(void) { display()->showFoundBlockOverlay(); }
void disp_show_error(const char *text, uint32_t code) { display()->showError(text, code); }
void disp_hide_error(void) { display()->hideError(); }

void disp_update(int toggle)
{
    display()->updateGlobalState(toggle);
    display()->updateCurrentSettings(toggle);
    display()->refreshScreen();
}
}
