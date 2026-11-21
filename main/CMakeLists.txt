# The fonts and images of the display (C arrays) and the web UI; the
# firmware itself is in components/rust_core.
idf_component_register(
SRCS
    "./displays/images/ui_font_DigitalNumbers16.c"
    "./displays/images/ui_font_DigitalNumbers28.c"
    "./displays/images/ui_font_OpenSansBold13.c"
    "./displays/images/ui_font_OpenSansBold14.c"
    "./displays/images/ui_font_OpenSansBold24.c"
    "./displays/images/ui_font_OpenSansBold45.c"
    "./displays/images/ui_font_vt323_21.c"
    "./displays/images/ui_font_vt323_35.c"
    "./displays/images/ui_img_found_block_png.c"
    "./displays/images/ui_img_safe_png.c"
    "./displays/images/ui_img_eth_png.c"
    "./displays/images/ui_img_can_png.c"
    "./displays/images/ui_img_wifi_png.c"
    "./displays/images/themes/themes.c"

INCLUDE_DIRS
    "displays/images"

REQUIRES
    "lvgl"
    "rust_core"

)


set(WEB_SRC_DIR "${CMAKE_CURRENT_SOURCE_DIR}/http_server/axe-os")

option(BUILD_WEB "Enable web UI build" ON)

if(BUILD_WEB)
    message(STATUS "Web UI build enabled")

    if("$ENV{GITHUB_ACTIONS}" STREQUAL "true")
        spiffs_create_partition_image(www ${WEB_SRC_DIR}/dist/axe-os FLASH_IN_PROJECT)
    else()
        find_program(NPM_EXECUTABLE npm)
        if(NOT NPM_EXECUTABLE AND NOT EXISTS ${WEB_SRC_DIR}/dist)
            message(FATAL_ERROR "npm is not found! Please install it to proceed.")
        endif()

        ExternalProject_Add(
            web_ui_dist
            PREFIX ${CMAKE_BINARY_DIR}/web_ui_dist
            SOURCE_DIR ${WEB_SRC_DIR}
            DOWNLOAD_COMMAND ""
            CONFIGURE_COMMAND ${NPM_EXECUTABLE} i
            BUILD_COMMAND ${NPM_EXECUTABLE} run build
            INSTALL_COMMAND ""
            BUILD_ALWAYS OFF
            BUILD_IN_SOURCE TRUE
        )

        add_dependencies(${COMPONENT_LIB} web_ui_dist)

        spiffs_create_partition_image(www ${WEB_SRC_DIR}/dist/axe-os FLASH_IN_PROJECT DEPENDS web_ui_dist)
    endif()
else()
    message(STATUS "Web UI build disabled – using prebuilt dist/ directory.")
    spiffs_create_partition_image(www ${WEB_SRC_DIR}/dist/axe-os FLASH_IN_PROJECT)
endif()
