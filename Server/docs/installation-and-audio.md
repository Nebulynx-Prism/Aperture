# Aperture installation and audio

Build from Server with `./tools/build_installer.ps1`. The script builds the Rust binaries and runs the local NSIS compiler. `-SkipBuild` packages the existing release binaries. It verifies FFmpeg 9.0.2 essentials against the pinned archive SHA256 before staging the encoder and its license/readme. A first build requires internet for this encoder download; later builds reuse the workspace cache. Build the native OpenVR driver separately with `./tools/aperture_driver/build.ps1` when its sources change.

The setup installs Aperture Server, Aperture Service, the portable encoder and Aperture's own SteamVR driver. SteamVR must be installed for driver registration. If registration is unavailable, setup keeps the driver files and provides a Register Aperture Driver shortcut to retry. OpenXR applications can use SteamVR's OpenXR runtime. The additional experimental OpenXR component installs the existing adapted runtime DLL, manifest and MIT license without changing the active runtime during installation. In Settings, select SteamVR OpenXR (SteamVR) or Aperture OpenXR (ApertureXR), then Apply runtime. This explicit action uses Aperture Service to set the 64-bit system OpenXR runtime; do not run both compositor backends concurrently.

VB-Audio VB-CABLE is an unchecked optional component. When selected, setup downloads the official Pack45 ZIP, verifies pinned SHA256 and the BUREL VINCENT publisher signature, then opens the vendor setup UI. Follow the vendor's license and restart instructions. It is not silently installed, and cancel/failure is reported separately from core installation. The cable is shared system audio infrastructure and is deliberately not removed when Aperture is uninstalled.

For normal desktop audio, use Headset and Computer and the Windows default playback endpoint. For isolated audio, route the game/application to **CABLE Input (VB-Audio Virtual Cable)** in Windows Sound settings, select Capture VB-CABLE in Aperture, save, and restart Aperture Server. CABLE Output is the recording side, not the playback endpoint used by this loopback capture. Headset Only requires the cable and reports an error rather than falling back to speakers when it is missing. Aperture does not change global default devices, mute speakers, enable Windows Listen, or install audio drivers at application startup.

Audio runs on a dedicated COM thread, converts float mix PCM to the client's 48 kHz stereo signed 16-bit little-endian format, and sends short stereo-aligned packets. Mono is duplicated; multichannel mixes currently use front left/right rather than surround downmix. Non-float/unsupported WASAPI formats fail explicitly. The VB-CABLE option supports desktop/game sound; headset microphone transport is not added by this change.

UI names use Aperture. PrismAperture class names, IPC identifiers, package IDs and existing executable names remain unchanged. Setup has not been executed during development: machine service installation, runtime registration, firewall updates and optional VB-CABLE installation occur only when the user runs it.

Sources: [VB-CABLE official download and instructions](https://vb-audio.com/Cable/), [FFmpeg build/download details and GPLv3 information](https://www.gyan.dev/ffmpeg/builds/). The FFmpeg package README contains build/source details; retain all bundled license notices when sharing the installer. Third-party code keeps its existing license terms.

## Server updates

The server's Updates tab keeps a manual check and a saved Auto update checks toggle. Enabled checks run at startup and every six hours against the public latest stable release in Nebulynx-Prism/Aperture. Turning checks off prevents future scheduled checks; a check already in progress may finish. A newer stable semantic-version tag is required; drafts/prereleases and equal/older versions are ignored.

The exact package names are **Aperture-UpdatePackage.zip** and **Aperture-UrgentUpdate.zip**. If both exist, urgent takes priority. A normal package shows `vX.Y.Z is available. View on GitHub for the changelog. Wanna update?` with **Update** and **Skip**, plus a GitHub changelog link. Skip is saved for that normal version. A manual check can re-offer it. Urgent packages ignore normal-version skips, start without an application confirmation, and show `An urgent update was pushed and your software was auto-updated due to the urgent update.` after verified installation and restart. Urgent detection still depends on an enabled automatic check or a manual check; disabling checks is respected.

`Aperture-Updater.exe` is built separately and included in setup. The server copies it into an isolated temporary job directory before launching it, so setup can replace the installed updater itself. The worker re-fetches the authoritative GitHub release, validates the repository URL, version and SHA256 digest (GitHub asset digest or its companion .sha256 asset), then validates the ZIP and embedded installer manifest. Only then does the server close and let the updater apply Aperture-Setup.exe silently. The updater verifies the installed server hash before restarting it. Preparation failures keep the original server open; install failures attempt to restart the available server and do not claim success. Windows administrator/UAC approval may still be required. Full rollback of a partially failed installer is not implemented; files held open by VR applications may cause failure.

ZIP extraction rejects traversal/absolute paths, links, duplicate required entries, invalid version/hash metadata and size-limit violations. Limits: 256 MiB download/installer, 512 MiB total expanded metadata, 128 entries, 8 KiB manifest, bounded extraction time. No code from the package is launched before verification. This authenticates downloads through the configured GitHub repository and HTTPS; it is not independent publisher code signing or protection against a compromised repository/local account.

### Publishing update packages

1. Bump Server/Cargo.toml to a newer stable semantic version and build `./tools/build_update_package.ps1`. The default builds fresh binaries/installer, then outputs target/release/Aperture-UpdatePackage.zip and its .sha256 file. `-SkipBuild` is only for already matching fresh binaries.
2. Use `./tools/build_update_package.ps1 -Urgent` for Aperture-UrgentUpdate.zip and its .sha256 file. Publish an urgent package only when automatic installation and interruption of the running server are intended.
3. Upload the selected ZIP and matching .sha256 file to a stable GitHub release whose tag matches the package version, e.g. v0.1.1. Uploading both ZIPs deliberately selects urgent behavior. No releases were uploaded by this task.

Each ZIP contains exactly the required root entries Aperture-Setup.exe and update-manifest.json. The manifest has version, setup_sha256 and server_sha256. Do not upload an arbitrary direct-file patch ZIP; use the producer script. Legacy Aperture-Setup.exe release assets still provide a browser download link when no update ZIP exists.

## Build validation (2026-11-07)

- Server/service offline release compilation passed, including the update controls.
- Offline unit tests: 39 server tests passed, 2 ignored; 4 service tests passed (43 passed total).
- Installer PowerShell scripts parsed successfully. VB-CABLE helper's VerifyOnly mode verified the downloaded Pack45 archive hash and publisher without launching setup.
- SteamVR registration helper passed DryRun discovery; it did not execute vrpathreg or change registration.
- Client APK rebuilt with its existing debug signing identity; apksigner verified the signature, and aapt reports application label Aperture with the existing org.nebulynx.aperture package ID.
- NSIS compiler produced target/release/Aperture-Setup.exe. Exact output hashes and payload identities are in installer-build-manifest.json. The installer and payload have not been deployment-tested on a clean PC. Live headset audio and live GitHub release availability have not been validated by these offline tests.


## Console and runtime settings

Normal server launches use the Windows GUI subsystem and do not create a command window, in debug or release builds. In Settings, Show diagnostic console window opens/closes a console immediately and saves the preference (off by default). Use the toggle to close it; closing a Windows console using its window close button can terminate its process. `--console` opens diagnostics for that launch. `--headless` and `--configure-pcvr` attach to the invoking terminal when available and preserve redirected stdout/stderr. The Windows service remains a separate SCM executable.

Settings offers SteamVR OpenXR (SteamVR) and Aperture OpenXR (ApertureXR). Startup and dropdown selection alone never change the runtime. Apply runtime runs on a worker thread through Aperture Service. The service verifies an absolute JSON manifest, format version and existing runtime library, then preserves the prior ActiveRuntime value in AperturePreviousRuntime before setting the new value in the 64-bit HKLM OpenXR key. Restore previous runtime restores that saved value (or removes ActiveRuntime if none existed originally). Registry mutations were not executed during build/testing.

Install the OpenXR component to make ApertureXR available. Close VR apps and SteamVR before switching, then restart your OpenXR applications. A per-process XR_RUNTIME_JSON override takes precedence over global selection. ApertureXR is experimental and intended for 64-bit OpenXR applications; OpenVR applications still need SteamVR. While ApertureXR is active, headset SteamVR launch requests receive instructions to start an OpenXR application directly rather than forcing the SteamVR compositor. The provider DLL's internal implementation identity remains unchanged; the selection manifest is aperture-xr.json, named ApertureXR.

Uninstallation restores the saved runtime if the currently selected manifest is Aperture's installed aperture-xr.json before removing its files. It does not undo an independently selected external runtime. Console and runtime settings support legacy settings: missing console preference defaults off, and the old OpenReality runtime value migrates to ApertureXR without erasing unrelated preferences.

Current validation: 39 server tests passed with 2 ignored, and 4 service tests passed (43 passed total). This includes console preference/migration checks and runtime manifest/library validation without registry writes. Live global runtime switching, interactive console toggling and installation on a clean PC were not exercised.

For ApertureXR streaming, request PCVR from the headset first, then launch the OpenXR app directly; that request arms the video path without starting SteamVR.



ZIP-updater validation: 43 Rust tests passed, 2 ignored, plus three PowerShell validation-only archive tests (valid package, traversal rejection and hash rejection). The updater and installer are rebuilt together. No release was published and no real update installation, UAC flow or server replacement was executed during development. Failed installations restart the available server with an attempt key to avoid an immediate urgent-update loop; Check for updates allows an explicit retry.


