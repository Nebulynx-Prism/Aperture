# SteamVR integration findings

## Evidence and cause

Prism's previous bridge created `VirtualDesktop.Oculus.DriverData` and body-state mappings, wrote unexplained offsets (0x188, 0x18c, 0xf0), and signaled events. There is no implemented ABI contract or corresponding consumer in Prism. It also forced `forcedDriver=null` and enabled `driver_null`: a simulated HMD cannot connect the Quest or deliver PCVR.

The bundled `tools/openvr_driver/driver.vrdrivermanifest` names `alvr_server`. Read-only GNU objdump inspection of its x64 DLL found `HmdDriverFactory`, `alvr_initialize`, `alvr_start_connection`, `alvr_poll_event`, `alvr_get_device_motion`, `alvr_send_video_nal`, and `VideoSend` exports. SHA256: `57C000C4642AD4466AF2BA9FADB19CD3734FB20EF61259B3013B82F6184096E0`. Export names do not establish signatures, initialization ordering, or interoperability. No DLL was loaded or executed. REA MCP tools were unavailable; these are ordinary source/PE observations, not REA runtime evidence.

[Valve's driver API](https://github.com/ValveSoftware/openvr/blob/master/docs/Driver_API_Documentation.md) requires a driver factory/provider and tracked device implementation. [ALVR's architecture](https://github.com/alvr-org/ALVR/blob/master/wiki/How-ALVR-works.md) describes the driver as the server responsible for discovery and streaming. [ALVR server C API source](https://github.com/alvr-org/ALVR/blob/master/alvr/server_core/src/c_api.rs) illustrates the separate lifecycle/tracking/video interface; current upstream source is context, not verified headers for this bundled nightly.

`src/session.rs` accepts Prism control connections and streams DesktopCapture frames. Desktop connection flags and packet counts do not establish an OpenVR device, pose delivery, or compositor submission. The former window-title watcher has been removed by parent integration. Registering ALVR cannot convert this protocol into ALVR. This explains the missing integration; the exact installed SteamVR Error 108 cause was not observed in a live runtime.

## Changes

- Bridge is inert, exposes `DriverStatus::Unavailable`, and warns explicitly. No shared-memory impersonation, unsafe Send/Sync, null-driver forcing, or SteamVR settings writes.
- Installer no longer packages/registers the unrelated ALVR server or unregisters/deletes a legacy driver on uninstall. Existing unrelated dirty installer changes retained.
- Service/client use newline-delimited JSON with 4096-byte bounds, handling fragmented byte-pipe reads. Service transaction deadline is five seconds. The client uses PIPE_NOWAIT byte mode with one two-second deadline across busy-pipe retries, partial request writes, and response reads; it polls at up to five-millisecond intervals and closes its handle on return. No flush that waits for the server to consume data is issued. Its public synchronous API is preserved; timeout makes ping return false and other operations return an error. Calls still belong off UI threads when a two-second pause is undesirable. Both binaries must be upgraded together; legacy unframed clients are not supported.
- Service accepts local pipe clients only and permits subsequent instances while a previous transaction finishes. Idle accept is bounded for shutdown polling. SCM failure is returned instead of silently running interactively; unknown CLI flags fail. Parent now implements throttling control using SetProcessInformation(ProcessPowerThrottling), state Version 1, ControlMask EXECUTION_SPEED and StateMask 0; PID zero is rejected. Parent will re-run the updated service tests. Priority is documented as HIGH, matching implementation.

## Parent integration status and remaining gaps

Parent integration has removed automatic SteamVR driver registration, null-driver configuration calls, and the window-title VR watcher from `src/session.rs`. LaunchSteamVr/LaunchGame requests now report PCVR unavailable instead of launching a runtime or switching the client into VR. Session sibling tasks are aborted and joined on termination. These items are completed, not current blockers. This follow-up changed only the service client and this report.

A real backend remains necessary: integrate a pinned ALVR client/server protocol and matching lifecycle/API, or implement a Prism OpenVR driver/provider with tracked HMD/controllers, timestamps/prediction, compositor texture transport, encoding and haptics. Match exact build headers and ownership/threading contracts before calling exported symbols. Desktop capture remains a desktop feature.

Review privileged pipe ACL/client identity and command authorization before production. Remote rejection is not local authorization. The client now has its own bounded transaction deadline against a silent pipe; remaining registry/OVR/HAGS and SCM handlers still need error/lifecycle review. A timeout does not roll back a privileged command already received by the service, and the client does not automatically retry commands after transmission.

Historical null-driver settings and registrations are deliberately not repaired automatically. Parent startup still attempts an ADB reverse TCP tunnel; this is separate from SteamVR integration. No service installation, ADB command, registry/firewall mutation or installed-app modification was performed by this follow-up.

## Validation

Parent reports 25 passing unit tests (2 service, 23 server) with one ignored server test and headless testing. The earlier encoder compilation blockage is resolved and is not a current finding.

This follow-up adds two local named-pipe tests: a server holding an ephemeral uniquely named pipe open without responding must hit the client's 100 ms test deadline before server closure; a response split across writes must parse successfully before its deadline. These tests use a private in-process Tokio server thread, contact no installed service and require no installation or machine configuration change. The production timeout is two seconds; the tests call the same exchange implementation with test-specific deadlines.

Full suite results for this follow-up are recorded below. No live headset/SteamVR or installer execution was performed. NSIS compiler was unavailable on PATH during the original investigation. Full PCVR is not implemented or validated.

Follow-up offline suite results: server **25 passed, 0 failed, 1 ignored** (including both new client tests); service **2 passed, 0 failed**. Total **27 passed, 1 ignored**. Existing compilation warnings remain. Scoped git diff --check passed. The timeout test completed within its asserted two-second upper bound while the server pipe remained open; both new tests passed in the complete server suite.

