# Prism native OpenVR backend

Build: `./tools/prism_openvr_driver/build_driver.ps1`.
Probe: `./tools/prism_openvr_driver/test_driver.ps1` (stop the Prism server first;
the probe refuses to replace existing IPC objects).

The distributable root is `driver/`. It includes the manifest, controller input
profile and `bin/win64/driver_prism_aperture.dll`. Valve SDK v2.12.14 header and
license are in `vendor/`. No installation, registration or SteamVR settings are
changed by these scripts.

Reference: Valve's official [IVRVirtualDisplay documentation](https://github.com/ValveSoftware/openvr/blob/v2.12.14/docs/Driver_API_Documentation.md#ivrvirtualdisplay).
The manifest redirectsDisplay flag and separate DisplayRedirect device are
required by that API. The HMD owns display geometry and real tracked pose;
the redirect device supplies the final composited SBS frames. The HMD also
exposes the virtual display interface for runtimes that query it directly.
There is no invented display-redirection property or foreign IPC layout.

The server must create the named mapping and mutex defined in prism_ipc.h before
SteamVR loads this driver, and keep them alive until the driver unloads. The
driver opens them, validates magic/version, and updates ready/heartbeat under
the mutex. A session is live only while client_connected is set and both PC
server heartbeat and tracking receipt are less than 500 ms old. Client monotonic
sample_time_us is deliberately not compared to Windows time. Invalid tracking
never creates a stationary fallback HMD. Without floor bit 0, every valid pose
gets +1.6 meters in Y. Coordinates are assumed to already use OpenXR/OpenVR
right-handed meters, -Z forward. FOV angles become raw tangents, with top/bottom
Y signs adjusted for OpenVR; IPD updates eye-to-head translations.

Capture opens PresentInfo_t.backbufferTextureHandle as a D3D11 shared resource
on the default hardware adapter. Only 2048x1024, single-sample, single-mip,
single-slice RGBA8/BGRA8 (including sRGB variants) are accepted. A keyed mutex
is mandatory. AcquireSync(0,10) must return S_OK: timeout/abandonment never permits
copy or release. After CopyResource, synchronous staging Map waits for GPU
completion before ReleaseSync(0). Successful mapped rows are copied respecting
RowPitch and RGBA is swizzled to BGRA. Pixels and frame metadata/sequence commit
under the named IPC mutex. Failures never increment frame_sequence. Only actual
Present calls can produce frames. SteamVR receives diagnostics through
IVRDriverLog (vrserver/vrcompositor logs); repeated capture errors are throttled.

Limitations for integration:
- Synchronous CPU Map/copy inside Present stalls the compositor; WaitForPresent
  does no additional work. This is a minimal backend, not a low-latency pipeline.
- Timing reports elapsed time since a successfully published compositor frame,
  with compositor frame ID. It has no headset scanout feedback or prediction.
- Default adapter only; hybrid-GPU adapter mismatch is logged, not recovered.
- No NT-handle fallback, device-loss recovery, scaling, HDR, MSAA or audio/haptics.
- Custom controller profile has no application default bindings, skeleton or
  render models. Users/apps need bindings for the exposed buttons and scalars.
- Server IPC objects cannot be recreated while SteamVR is running: restart the
  driver after server replacement. Mapping creation is exclusively server-owned.

The probe validates ABI compile assertions, real freshness/floor conversion,
invalid quaternion rejection, disconnected tracking, display geometry/distortion,
IPC ready/heartbeat lifecycle, DLL loading and factory export lookup. It does not
exercise SteamVR device activation or capture a live compositor texture. Those
require the parent integration test with SteamVR and a connected Quest.
