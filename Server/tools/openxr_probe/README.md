Windows OpenXR diagnostic and bounded D3D11 smoke application. Uses the project's
OpenXR headers and w64devkit; no OpenXR import library or installation required.
All source and build output stay in this directory.

Build from the repository root:

```powershell
& ./tools/openxr_probe/build.ps1
```

Parent-run diagnostics after the backend is ready (these commands were not run
during implementation):

```powershell
./tools/openxr_probe/target/openxr-probe.exe --runtime 'C:\Program Files (x86)\Steam\steamapps\common\SteamVR\steamxr_win64.json' --probe
./tools/openxr_probe/target/openxr-probe.exe --runtime 'C:\Program Files (x86)\Steam\steamapps\common\SteamVR\steamxr_win64.json' --render-seconds 10
```

`--probe` is the default. Rendering requires an explicit integer duration 1..30;
10 seconds is the suggested smoke duration. Modes are mutually exclusive.
An optional `--loader` must resolve to the same installation's
`bin/win64/openxr_loader.dll`. Only this absolute DLL is loaded, with dependency
search restricted to its directory and System32. Select a trusted SteamVR
installation: directory placement is checked, publisher signatures are not.
`XR_RUNTIME_JSON` is set only in the probe process. No registry/configuration,
driver registration, SteamVR launch, or installation is performed by this app.
OpenXR calls may themselves activate runtime services according to SteamVR's
behavior; invoke only when the parent is ready for a runtime session.

Output includes every required function resolution, runtime identity, actual HMD
system properties, view configurations, session state transitions, accepted
projection frame count, zero-layer frames, valid/tracked head pose counts, pose
changes, and maximum consecutive position/orientation changes. GetSystem failure
before a head device connects is an error, never a healthy registration result.
Probe mode proves only instance/system availability. Render mode requires a real
session, accepted projection frames, and valid head poses. Stationary poses do
not fail the test; tracked flags and changes are reported separately.

The render loop waits for READY, begins PRIMARY_STEREO, uses LOCAL and VIEW
spaces without actions, and clears a separate swapchain per eye to subtly
animated neutral colors. Frames with shouldRender=false or invalid view poses
end with zero layers. Image waits have a one-second timeout. Rendering lasts at
most the requested loop duration; initialization/cleanup can add time. An overall
watchdog terminates a stalled process after duration + 15 seconds (probe: 15
seconds). Forced termination cannot provide normal runtime cleanup. Otherwise,
partial initialization and failures release owned session, space, swapchain, COM,
DLL and watchdog handles; normal exit requests STOPPING near the deadline.

Exit codes: 0 successful selected mode, 1 runtime/render failure, 2 argument or
preflight failure, 3 watchdog timeout. API failures include numeric XrResult or
HRESULT. A cleanup failure is logged separately.

For the driver's smoke test, require `projection_submissions > 0` **and** an
independent increase in actual driver frame-export counters or captured frames
during this run. Verify the neutral animation in the exported frames. This app's
successful xrEndFrame calls alone cannot prove driver export, Rust transport,
Android receipt, or headset display. Correlate those pipeline observations in
the parent test; the probe deliberately does not alter that implementation.
