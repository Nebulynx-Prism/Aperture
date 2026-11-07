# Prism Aperture investigation and repair

Investigation date: 2026-11-06. Existing working-tree changes were preserved.

## Verified desktop failure and repair

The live Quest 2 connected and initialized MediaCodec while its desktop stayed black. The PC log reported a 1536×960 capture on a 1920×1200 display. DXGI desktop coordinates were DPI-virtualized; the code allocated a staging texture from those coordinates, then copied a physically sized resource into it. Capture now uses the duplication mode's physical dimensions and checks incoming texture dimensions. The GDI fallback now deselects its bitmap before GetDIBits, checks BitBlt/readback failures, and restores selected objects before cleanup.

After rebuilding, the first desktop capture contained 230 nonblack aggregate samples. The user confirmed **“Desktop is visible now.”** This confirms a visible desktop in the actual OpenXR client, not merely a successful connection.

## Transport and decoding

- USB uses framed TCP video through ADB reverse on TCP 38810. Startup no longer restarts adbd into TCPIP mode or attempts UDP forwarding through ADB.
- Wi-Fi video uses ORVD datagrams capped at 1200 bytes, with bounded reassembly, duplicate/header checks, sequence reset, and keyframe recovery. Audio and video have separate sequence counters.
- StreamConfig (102) supplies codec/width/height before video. StreamConfigAck (103) confirms successful decoder setup; the server waits for this before sending frames. StreamStatus (104) carries UTF-8 status text. Both updated endpoints are required.
- TCP reads/writes handle partial operations; socket shutdown joins workers before descriptor reuse. Server handshakes and writes have deadlines. Discovery publishes real interface addresses and repeats UDP scans.
- MediaCodec output is drained independently of input arrival. Ordinary surface output indices are rendered even when byte-buffer size is zero. The renderer retains desktop mode when no negotiated PCVR stream exists.
- FFmpeg output is split into complete H.264/HEVC access units, with bounded queues and errors instead of fake raw-BGRA “H.264.” Static desktops keep feeding the encoder. Repeated recovery requests are rate-limited to avoid restart starvation. Automatic currently selects H.264; AV1 requests explicitly fall back to H.264 until AV1 framing is implemented.
- Session teardown stops sibling tasks and avoids polling an already-consumed JoinHandle. Windows listeners no longer enable SO_REUSEADDR, which allowed overlapping live streamers to bind one port during diagnosis.

## Live Quest transport checks

The standalone diagnostic uses the connected Quest 2's Qualcomm hardware decoder, SurfaceTexture, and offscreen EGL. It saves counts only, not desktop images. These eight-second runs occurred alongside the app, so their rates are not single-client performance benchmarks.

| Transport | Complete video frames | Hardware output / surface frames | Nonblack aggregate samples | Expired / invalid / gaps |
| --- | ---: | ---: | ---: | --- |
| USB TCP | 227 | 227 / 227 | 51,529 / 58,112 | 0 / 0 / 0 |
| Wi-Fi UDP | 187 | 187 / 187 | 42,472 / 47,872 | 0 / 0 / 0 |

Both configured and output 1920×1200 H.264. Logs: `target/live-usb-probe.log`, `target/live-wifi-probe.log`, and `target/live-quest-desktop-access.log`. Earlier failed runs are diagnostic history, not the successful results above. Desktop capture requires an interactive desktop; the sandbox initially denied DXGI access, so successful capture tests ran with approved desktop access.

## Cursor and trigger follow-up

The user reported cursor movement reaching Windows while the cursor was invisible, and requested a trigger tap for left-click and a hold for right-click. Cursor compositing adds the OS cursor using a small reusable bitmap and restores only its touched rectangle after encoding, so unchanged desktops do not accumulate cursor trails. Mouse packets use an explicit 17-byte layout; server injection honors the absolute flag, rejects malformed values, and sends button events only when state changes. Client gestures use a FIFO so a complete click's down/up cannot collapse into one snapshot. Hold recognition suppresses the tap click; menu interaction, tracking loss and disconnect cancel desktop gestures.

The original aiming reticle could also be occluded by the curved desktop because hit testing used a flat plane. Pointer drawing now bypasses depth testing and restores the previous depth state afterward. After installing the final APK, the user confirmed **“Cursor and both clicks work.”** The gesture threshold is 500 ms: shorter presses left-click on release; holds produce a single right-click without a later left-click. Curved-screen hit-coordinate refinement remains separate from this visibility fix.

Final verification: **31 Rust tests passed**, with the installed-FFmpeg test ignored in the ordinary suite and separately run successfully for both H.264 and HEVC. The host C++ gesture suite passed **50 timing, cancellation, FIFO and normalized-coordinate checks**. The final native Android library linked, zip alignment passed, APK v3 signing verified, and ADB replacement installation succeeded without uninstalling or clearing app data. Existing Rust/C++ warnings remain.

Final build identities:

| Output | SHA256 |
| --- | --- |
| `client/PrismAperture.apk` | `52003254D3C9D870B8BC9FC2FE5D64EC169CA8DF19551BEDC436C3E4DDDAEB07` |
| `target/release/prism-aperture-server.exe` | `697E7E8940108C595E3786431BDC6EC26BB8B42B41F41037EC6FEA1EE6B5DAEA` |
| `target/release/prism-aperture-service.exe` | `72CA411B4C230625D6876CD46041486E4760A43B72930DAE79D2827DB5A6AA09` |

The client is locally debug-signed. The final server tests used a console process that stops automatically after 55 seconds; launch the release server executable for normal use.

## Service and PCVR boundary

The service pipe has bounded newline framing and transaction deadlines. A live local service/server smoke test confirmed ping, high process priority, and actual execution-speed throttling control through SetProcessInformation. No persistent Windows service was installed during testing.

**Full PCVR remains unimplemented.** The old bridge fabricated Virtual Desktop shared-memory state and bundled an ALVR driver without implementing that driver's client/server protocol. Desktop connection and launching a SteamVR process do not create a tracked HMD. The fake bridge, automatic unrelated driver registration and null-HMD configuration were removed. The client explicitly shows PCVR unavailable and keeps the desktop visible. A real tracked-device/compositor backend is required before SteamVR headset detection or VR game streaming can work.

## Reference analysis and reproducibility

- [Reference artifact inventory](reference-artifact-findings.md): APK manifest/native/DEX inventory, hashes, PE headers and exports.
- [Managed reference analysis](managed-reference-findings.md): reflection-only metadata and bounded IL for 61 methods across the three installed Windows assemblies. No target code was invoked.
- [SteamVR findings](steamvr-findings.md): backend boundary and service details.

REA and decompilation engines were unavailable. Local aapt, objdump and reflection-only inspection supplied the reference evidence; no Virtual Desktop networking/driver ABI is claimed recovered. Prism's wire protocol is its own implementation.

The Android packaging path now builds the project sources directly using installed SDK/NDK/JDK tools; it no longer starts from or repackages the Virtual Desktop APK. Run `client/scripts/package_apk.ps1`. Rust release outputs are built with `cargo build --offline --release --bin prism-aperture-server --bin prism-aperture-service`. A bounded console diagnostic is available with `prism-aperture-server.exe --headless --run-seconds 55`.

The supplied desktop artifacts are development builds. The NSIS installer source was updated, but the historical installer executable was not rebuilt or run. Multi-monitor cursor mapping, audio streaming, VR tracking/controllers/haptics, AV1 framing and full PCVR are outside the verified results.
