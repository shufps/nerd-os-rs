/*
 * The ESP-IDF headers of the Rust code (with bindings.h): bindgen generates
 * src/idf/raw.rs from this file's includes and flags (build.rs), so the
 * Rust code uses the real struct layouts and constants (`idf_const!`). What
 * remains checked here is the configuration the Rust code relies on.
 */

#include <stddef.h>

#include "driver/adc.h"
#include "driver/gpio.h"
#include "driver/i2c.h"
#include "esp_adc_cal.h"
#include "esp_app_desc.h"
#include "esp_heap_caps.h"
#include "esp_transport.h"
#include "lwip/sockets.h"
#include <errno.h>
#include "esp_http_client.h"
#include "esp_sntp.h"
#include "esp_system.h"
#include "esp_http_server.h"
#include "esp_vfs.h"
#include "esp_partition.h"
#include "esp_ota_ops.h"
#include "esp_spiffs.h"
#include "esp_wifi.h"
#include "lwip/stats.h"
#include "ping/ping_sock.h"
#include "esp_task.h"
#include "driver/twai.h"
#include "driver/spi_master.h"
#include "esp_eth.h"
#include "esp_eth_mac_spi.h"
#include "esp_netif.h"
#include "esp_event.h"
#include "esp_timer.h"
#include "esp_task_wdt.h"
#include "mbedtls/x509_crt.h"
#include "esp_mac.h"
#include "esp_intr_alloc.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include <sys/time.h>

/* src/wifi_health.rs */
#if !(IP_STATS && TCP_STATS && UDP_STATS && ICMP_STATS && IPFRAG_STATS && ETHARP_STATS && LINK_STATS)
#error "src/wifi_health.rs logs all lwIP protocol stats"
#endif

/* TWAI_TIMING_CONFIG_500KBITS() of the ESP32-S3 (APB): src/idf/twai.rs */
#if SOC_TWAI_CLK_SUPPORT_XTAL
#error "TWAI_TIMING_CONFIG_500KBITS() differs with an XTAL clock source: update src/idf/twai.rs"
#endif
