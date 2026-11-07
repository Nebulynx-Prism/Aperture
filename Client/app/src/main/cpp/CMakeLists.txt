cmake_minimum_required(VERSION 3.22.1)
project(prism_aperture_client LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

include_directories(
    ${CMAKE_CURRENT_SOURCE_DIR}/include
)

# Prefer the checked-out local loader for the standalone developer build. Clean
# GitHub builds resolve the Khronos loader AAR through Gradle Prefab instead.
set(OPENXR_LOADER_LIBRARY "${CMAKE_CURRENT_SOURCE_DIR}/../jniLibs/${ANDROID_ABI}/libopenxr_loader.so")
if(EXISTS "${OPENXR_LOADER_LIBRARY}")
    add_library(openxr_loader SHARED IMPORTED)
    set_target_properties(openxr_loader PROPERTIES IMPORTED_LOCATION "${OPENXR_LOADER_LIBRARY}")
    set(OPENXR_LOADER_TARGET openxr_loader)
else()
    find_package(OpenXR REQUIRED CONFIG)
    set(OPENXR_LOADER_TARGET OpenXR::openxr_loader)
endif()

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
    ${OPENXR_LOADER_TARGET}
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
