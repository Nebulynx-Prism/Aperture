# Prism VDXR backend port - compiled and smoke-tested, uncommitted

All port changes remain reviewable in tools/prism_vdxr/. No Git commit was
created. Downloaded upstream, dependencies, test executables and build outputs
are locally ignored. No global OpenXR runtime setting was changed.

Upstream VDXR commit: `6da20fe478e331c05672243fd82a9a1500cd39f5`.
The MIT-licensed VDXR runtime is adapted by replacing `OVR_CAPIShim.c` with
`prism_backend.cpp`. It does not load Virtual Desktop or Oculus runtime binaries.
The original license is copied into bin/LICENSE-VDXR.txt. Quest continues to use
Meta OpenXR and Prism's existing TCP protocol.

## Implementation

- Uses the existing server-owned `Local\PrismAperture.OpenVR.v1` mapping and
  `Local\PrismAperture.OpenVR.Mutex.v1` mutex from authoritative `prism_ipc.h`.
- Real head/grip poses, normalized quaternions, tracking validity, controller
  buttons/sticks/trigger/squeeze, eye FOV, IPD and measured refresh. Floor/eye
  conversion follows Prism's existing 1.6 m convention. No fabricated tracking,
  and no comparison of client monotonic timestamps with PC time.
- D3D11 shared buffered swapchains, sRGB composition, per-eye viewports,
  rotational projection reprojection and spatial/head-locked quad composition.
- Synchronous GPU readback respects row pitch. Publishes 2048x1024 BGRA SBS
  pixels, dimensions, stride, format, frame timestamp/sequence, and fresh
  driver_ready/heartbeat together under the existing mutex.
- Empty/invalid submissions do not increment frame sequence. Stale tracking
  removes headset availability. Haptics/audio-device selection are unsupported.
- Optional body/face/eye/hand sensors and cylinder extension are disabled in the
  adapted build. Async resubmission and mirror-window output are disabled.
- A dedicated submission device avoids changing application graphics state.

Only one compositor backend may write the production frame slot at a time.
Do not run the OpenVR driver and Prism VDXR concurrently against this mapping.
Unit tests use isolated names; runtime fixture testing refuses any live mapping.

## Build and run

1. `./tools/prism_vdxr/fetch_upstream.ps1`: pinned source/dependencies.
2. `./tools/prism_vdxr/test_backend.ps1`: independent backend/GPU tests.
3. `./tools/prism_vdxr/build_runtime.ps1`: build bin/prism-vdxr.dll + JSON.
4. `./tools/prism_vdxr/test_runtime.ps1`: loader/session fixture test; no server.
5. Start server `--headless --pcvr`, connect/wake Quest, then use
   `test_runtime.ps1 -Live` or `run_app.ps1 -Application <OpenXR executable>`.

The launcher sets XR_RUNTIME_JSON for one application and its children, restoring
its prior value afterward. It never registers the runtime globally. Games started
through an already running Steam process may not inherit this variable; the
existing Android LAUNCH_STEAM_VR button still targets the unchanged SteamVR path.
Use the explicit application launcher for the VDXR path currently validated here.

## Verified on 2026-11-06

- The old MSVC/1618 blocker is resolved: Visual Studio Community 2026 18.11,
  MSVC 14.51 (v145), Windows SDK 10.0.26100.0 are present. The build wrapper now
  selects the installed toolset rather than requiring upstream's v143.
- Full VDXR runtime build/link PASSED: 0 warnings, 0 errors. Output:
  bin/prism-vdxr.dll (1,459,712 bytes) and bin/prism-vdxr.json. MSBuild required
  approved execution outside the sandbox because its shader file tracker was
  denied access inside the sandbox. No additional system install was performed.
- Backend test PASSED: session/IPC/freshness/origin/input/invalid-pose/cleanup;
  actual D3D11 red/blue eye textures verified as correct SBS BGRA pixels; a
  green spatial quad verified at center/corner pixels; empty submit has no frame.
  Also linked/reran with upstream OVR utility and stereo-projection source.
- Loader/session fixture PASSED: runtime identified itself as Prism VDXR 1.1.0,
  D3D11 session reached FOCUSED, 199 accepted projection submissions produced
  199 independently verified shared-memory SBS frames at 2048x1024.
- LIVE Quest test PASSED: installed app initially asleep / tracking unavailable;
  waking and foregrounding it restored real tracking. The test application
  accepted 238 projection frames with 238 valid/tracked head samples, including
  27 measured pose changes. No fixture tracking was used in this live run.
- Rust server independently logged stream generation 2, 2048x1024 SBS=true,
  driver=Ready, and 77.6-90.5 streaming FPS during the live run. Android logged
  SBS=1 generation=2 and hardware decoder output 2048x1024. On test app exit,
  server/client returned to desktop, generation 3, 1920x1200. The unchanged
  server's generic PCVR status text still says 'SteamVR compositor'; the actual
  producer in this test was the Prism VDXR DLL and no SteamVR process ran.
- Per-application launcher PASSED with the fixture test (199 exported frames).
  A later live invocation after the bounded server test had ended correctly
  returned XR_ERROR_FORM_FACTOR_UNAVAILABLE rather than inventing a headset.
- All six PowerShell scripts passed parser checks. Upstream preparation reruns
  from pinned originals; all three NuGet dependencies restored.

Evidence logs (workspace target/): vdxr-build-attempt.log,
vdxr-runtime-probe.log, vdxr-live-probe.log, vdxr-app-launch.log,
quest-pcvr-live-unrestricted.log. The bounded server test shut down cleanly.
The already installed Quest APK was used without reinstallation.

## Remaining limits

This verifies the D3D11 OpenXR smoke-test path through the real Rust/Quest stream;
it is not a game-compatibility certification or human confirmation of headset
visual quality. Real-game input bindings/haptics, sustained latency/comfort and
D3D12/Vulkan/OpenGL interop remain untested. No positional timewarp or velocity
prediction; CPU readback is synchronous. Final submitted color layers must be
single-slice/single-sample; VDXR resolves application arrays/MSAA upstream.
Unsupported formats/layers return errors.

No Rust/Android protocol modification was required for the ABI shortcut. No
OpenVR driver change, SteamVR restart or global runtime selection was performed
during this build-and-test retry. Changes remain uncommitted.
