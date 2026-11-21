/* The log over the web socket (src/http_ws.rs): the vprintf hook that
 * formats each log line, prints it as before and queues a copy for the
 * socket. C because the hook takes a va_list. */
#include <stdarg.h>
#include <stdbool.h>
#include <stdio.h>
#include <string.h>

#include "esp_heap_caps.h"
#include "esp_log.h"

/* copies the line (src/http_ws.rs); false if it was dropped */
bool ws_log_enqueue(const char *line, size_t len);

static int log_to_queue(const char *format, va_list args)
{
    va_list copy;
    va_copy(copy, args);
    int needed = vsnprintf(NULL, 0, format, copy) + 1;
    va_end(copy);

    // +2 for a newline and the NUL
    char *line = heap_caps_calloc(needed + 2, 1, MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT);
    if (line == NULL) {
        return 0;
    }
    va_copy(copy, args);
    vsnprintf(line, needed, format, copy);
    va_end(copy);

    // every line ends with a newline
    size_t len = strlen(line);
    if (len > 0 && line[len - 1] != '\n') {
        line[len] = '\n';
        line[len + 1] = '\0';
    }

    printf("%s", line);

    ws_log_enqueue(line, strlen(line));
    heap_caps_free(line);
    return 0;
}

void ws_log_install(void) { esp_log_set_vprintf(log_to_queue); }
void ws_log_uninstall(void) { esp_log_set_vprintf(vprintf); }
