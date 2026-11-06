cmake_minimum_required(VERSION 3.22.1)
project(prism_aperture_client LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

include_directories(
    ${CMAKE_CURRENT_SOURCE_DIR}/include
)

# Prebuilt OpenXR Loader library
add_library(openxr_loader SHARED IMPORTED)
set_target_properties(openxr_loader PROPERTIES
    IMPORTED_LOCATION "${CMAKE_CURRENT_SOURCE_DIR}/../jniLibs/${ANDROID_ABI}/libopenxr_loader.so"
)

add_library(prism_aperture_client SHARED
    src/main.cpp
    src/openxr_context.cpp
    src/vr_gui.cpp
    src/video_decoder.cpp
    src/audio_player.cpp
    src/network_client.cpp
    src/input_manager.cpp
)

target_link_libraries(prism_aperture_client PRIVATE
    openxr_loader
    EGL
    GLESv3
    mediandk
    aaudio
    jnigraphics
    android
    log
)

target_compile_options(prism_aperture_client PRIVATE
    -O3
    -fvisibility=hidden
    -fno-fast-math # Tracking/gesture validity depends on finite-value checks.
    -Wall
    -Wextra
)
