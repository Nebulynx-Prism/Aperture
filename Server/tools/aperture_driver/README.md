# Aperture driver

Independent Aperture Rust implementation of the PC OpenVR driver. ALVR and VDXR
are architectural references only: none of their implementation sources, runtime
DLLs, Android engines, or protocol libraries are dependencies of this driver.
The C++ shim implements Valve's public virtual interfaces using the public
OpenVR SDK header; it delegates tracking, timing, IPC, and capture to Rust.

OpenXR PC applications currently use the SteamVR OpenXR runtime. This directory
does not implement a separate native PC OpenXR runtime. The Aperture Android
client uses the headset's OpenXR runtime.

## Build and checks

From `Server`, with Rust's MSVC target and Visual Studio C++ tools available:

```powershell
./tools/aperture_driver/build.ps1
./tools/aperture_driver/test_abi.ps1
cd tools/aperture_driver
cargo test --offline --target x86_64-pc-windows-msvc capture::tests::isolated_shared_gpu_frame_exports_without_a_second_present -- --ignored --nocapture
```

Output: `driver/bin/win64/driver_aperture.dll`. Building does not register the
driver, change SteamVR settings, or launch SteamVR. The server's runtime setup
selects this driver when the updated server is explicitly run in PCVR mode.

The GPU test creates a private mapping and a real D3D11 shared texture. It checks
actual exported pixels after a single Present; it never opens the live server
mapping. The ABI test calls the compiled MSVC interfaces and checks struct-return
integrity without initializing a live driver. Neither test establishes headset
latency, game compatibility, or an operational OpenXR session.

## Data flow

The driver reads fresh Aperture tracking and observed device display properties.
Missing, invalid, or stale poses remain untracked. Actual OpenXR velocities and
target times travel in an optional versioned motion message. Four-timestamp
clock exchanges align the headset's monotonic time to host QPC; poses are not
extrapolated a second time in the driver.

Present callbacks replace pending raw texture descriptors. A worker owns the
D3D11 context, flushes GPU copies, polls completion without requiring another
Present, and exports completed BGRA pixels with their capture-time pose metadata.
It uses bounded staging resources and nonblocking mapping access. This queue
operates before encoding; it does not discard interdependent encoded pictures.

FOV comes from the headset. Recommended eye dimensions come from the existing
device configuration. The explicit display contract preserves the existing
2048x1024 side-by-side stream; this rewrite adds no FOV or resolution tuning.

## Remaining live validation and limitations

This driver must pass a real headset connection, SteamVR device activation,
OpenXR session submission, and end-to-end frame/motion checks before being
described as a working replacement. Isolated tests are insufficient.

Vsync currently follows a host cadence at the reported headset refresh rate;
it is not phase-locked to measured headset scanout. Legacy LOCAL tracking retains
the existing 1.6 m floor offset. Controller haptics are not implemented. These
limitations must not be presented as solved latency or complete compatibility.

The existing Aperture encoder and transport remain the streaming backend. This
is a driver rewrite with an independently defined motion/clock extension, not a
claim that every server and client subsystem has been rewritten.
