# Rust driver port status

Updated 2026-11-06.

## Implemented

- Replaced the previous native implementation with `rust_core`, a Rust `staticlib` compiled for the Windows MSVC ABI. The Rust crate uses the Windows crate for D3D11/DXGI.
- Moved Prism shared-memory/mutex open, validation, read, heartbeat, device metadata access, and SBS frame publication into Rust. It uses the existing `Local\\PrismAperture.OpenVR.v1` mapping and `Local\\PrismAperture.OpenVR.Mutex.v1` mutex; the 264-byte header, metadata offset 264, pose offset 280, pixel offset 4096, and BGRA frame format are preserved.
- Moved pose validation, OpenXR FOV-to-OpenVR projection conversion, adapter preference ranking, and pitch-aware BGRA/RGBA row copying into Rust.
- Moved tracking freshness evaluation and conversion of the real head/controller poses (including stage-origin height handling and quaternion normalization) into Rust.
- Moved the D3D11/DXGI implementation into Rust: adapter enumeration and fallback, shared-resource opening, keyed mutex handling, format/dimension validation, staging texture/query ring, nonblocking reads, and frame publication.
- Rust owns provider runtime state, heartbeat/mapping lifetime, tracking and input state, headset identity/configuration, projection/IPD/refresh updates, recommended resolution, distortion/viewport values, vsync timing, and GPU resources.
- `driver.cpp` is the MSVC OpenVR ABI adapter. It implements Valve's C++ virtual interfaces, registers the tracked-device objects, translates the Rust-owned state into OpenVR C++ structs, and calls OpenVR host/property/input interfaces. The C++ ABI entry points remain because Valve's server-driver interface is a C++ virtual interface with MSVC calling/return conventions.
- Added per-frame Android `SurfaceTexture.getTransformMatrix()` propagation and applied the matrix in both the SBS eye shader and desktop-screen shader. Android documents that this matrix must be re-queried after every `updateTexImage()` and applied before sampling.

## Build results

- Rust core: `cargo build --release --target x86_64-pc-windows-msvc` succeeded.
- Driver: `build_driver.ps1 -OutputDirectory tools/prism_openvr_driver/target/rust-staged` succeeded. Staged DLL: `target/rust-staged/driver_prism_aperture.dll`.
- A staged DLL build succeeded after the Rust D3D and runtime ports. The replacement at `driver/bin/win64/driver_prism_aperture.dll` matches the staged build (SHA-256 `D5ABA42AE3D02E92FD30BF678C0580350A745235A0675A90C2707866D2DB91B9`). No SteamVR process was running during replacement; SteamVR was not restarted.
- Quest client: offline Release packaging succeeded and signed `Client/PrismAperture.apk`. The APK was not installed and SteamVR was not restarted.
- No headset visual/runtime check was performed. No claim is made that the upside-down image is verified fixed on-device.

## Validation still required

- Run the existing OpenVR ABI smoke test and then a SteamVR launch test. No runtime test was performed in this turn.
- Visually validate orientation and video on the Quest, then run compatibility checks on Intel Arc, AMD Radeon, and NVIDIA GeForce. Adapter selection/retry is implemented, but source/build success does not validate each vendor's compositor-sharing behavior.
- SteamVR was not restarted, and the Quest image was not visually checked. The native build is complete; runtime validation is pending.

## Architecture notes from source review

- VDXR is Virtual Desktop's PC OpenXR runtime. It is not a SteamVR OpenVR hardware-driver implementation. This repository's VDXR fork is a separate runtime path that publishes into Prism IPC.
- Meta's Quest OpenXR runtime is what the Android client uses. OpenXR is the cross-vendor API and runtime contract; the loader dispatches to a runtime and is not itself a headset runtime.
- Xrizer is a Rust reimplementation of the OpenVR application API on top of OpenXR, primarily a compatibility layer for OpenVR applications. Its direction and host platform differ from Prism's Windows SteamVR server-driver role. Its design is useful reference material, but its code is not copied into this project.

References:

- [Xrizer source](https://github.com/Supreeeme/xrizer)
- [VDXR source](https://github.com/mbucchia/VirtualDesktop-OpenXR)
- [Meta OpenXR core concepts](https://developers.meta.com/vr/documentation/native/android/mobile-openxr-core-concepts/)
- [Android SurfaceTexture transform contract](https://developer.android.com/reference/android/graphics/SurfaceTexture#getTransformMatrix(float[]))
- [Valve OpenVR driver API documentation](https://github.com/ValveSoftware/openvr/blob/master/docs/Driver_API_Documentation.md)
