# Virtual Desktop reference artifact findings

Collected locally on 2026-11-06 using [the read-only collector](../tools/inspect_reference_artifacts.ps1). [Machine evidence](reference-artifacts.json) records full paths, SHA256, byte sizes, PE headers, bounded export rows, ZIP members, tool identities, and aapt output. These observations apply only to the hashed files; filenames and directory version labels do not establish authenticity.

## Static observations

### APK

Input: `C:\Rookie\Virtual Desktop v10708+1.34.22.0 -ZuRiUs\VirtualDesktop.Android.apk` (the matching local Virtual Desktop APK found during path discovery).

- Size: **1,115,340,924 bytes**.
- SHA256: `861FAD237A7EE85C6EAFE6A83BA5340854CAF8A79D51B0476FFCC674F60046FC`.
- ZIP has **948 entries**. All **205** members matching AndroidManifest.xml, root classes*.dex, or lib/**/*.so were inventoried and individually hashed without extraction: one manifest, one DEX, 203 native library entries. Other assets/resources were not individually inventoried or hashed; the full APK hash covers their container bytes.
- `aapt dump badging` returned package **VirtualDesktop.Quest**, versionCode **10708**, versionName **1.34.22.0**, minimum SDK **29**, target SDK **32**, compile SDK **35**, native ABI **arm64-v8a**.
- Launch activity: `md59102214312e19799944a61bf7bc2f23e.VrActivity`. The decoded manifest marks it exported and declares MAIN/LAUNCHER, XR and Oculus VR categories.
- `classes.dex` is **6,290,620 bytes**. Native members include `libmonodroid.so`, `libmonosgen-2.0.so`, `libxamarin-app.so`, `libopenxr_loader.so`, `libOVRPlatform64_1.so`, and `libaot-VirtualDesktop.{Android,Core,Mobile,Net}.dll.so`.
- Manifest declares a `mono.MonoRuntimeProvider`, supportedDevices `quest2|questpro|quest3`, optional head/hand/body/face/eye tracking features, and permissions including INTERNET, RECORD_AUDIO and Oculus tracking permissions. Declarations do not establish actual use, successful initialization, permission grants or hardware support.
- Both aapt operations exited **0**. The complete **266-line** decoded manifest fits within the collector limit; raw binary XML is represented by its ZIP-member hash, not copied out.

**Inference:** Mono provider declarations together with the Mono/Xamarin/AOT member names strongly suggest a .NET/Mono Android packaging layer alongside DEX and native components. This is a packaging inference, not recovered application logic. No methods, packet formats or native ABI were decompiled.

### Installed Windows artifacts

Recursively inspected `.exe`, `.dll` and `.sys` files under `C:\Program Files\Virtual Desktop Streamer` and `C:\Program Files\Virtual Desktop`: **36 files** with hashes and PE headers. This is a binary inventory, not an inventory of every installed asset, MSI or configuration file.

| Artifact (relative to its installation root) | Bytes | PE header observation |
| --- | ---: | --- |
| Streamer: `VirtualDesktop.Streamer.exe` | 22,614,552 | AMD64 / PE32+, CLR directory RVA 8192, size 72 |
| Desktop: `VirtualDesktop.Server.exe` | 7,451,672 | I386 / PE32, CLR directory RVA 8200, size 72 |
| Desktop: `VirtualDesktop.Service.exe` | 5,855,256 | I386 / PE32, CLR directory RVA 8200, size 72 |
| Streamer: `OpenVRDriver/bin/win64/driver_VirtualDesktop.dll` | 676,376 | AMD64 / PE32+, no CLR directory; named export `HmdDriverFactory` |
| Streamer: `OpenXR/virtualdesktop-openxr.dll` | 1,485,768 | AMD64 / PE32+, no CLR directory; 81 matched named export rows, including `xrNegotiateLoaderRuntimeInterface` |
| Streamer: `libVirtualDesktopNet.dll` | 137,752 | AMD64 / PE32+, no CLR directory; named exports `OptimizeWLAN`, `StartWLANOptimizerThread`, `StopWLANOptimizerThread` |

CLR directory presence is evidence of a CLI-bearing PE; managed assembly contents were not decoded. I386 in a CLI PE does not alone establish the process execution architecture (CLR flags were not inspected). No CLR directory does not rule out embedded managed payloads or NativeAOT.

Key SHA256 identities:

- Streamer: `0229A322137F55687240D2B51F5F7002056D9277D7FA001EC6CB4EB25AF0BFB8`.
- Server: `2C14AC126D176F0C7F3D3FE7ED61B0B5F26629153B51E3BCCA9C6E1E0AC8A11D`.
- Service: `4D50F085CEC054FC4990A6FD75BE59D1428423634088A86B920BFD85A1E653FC`.
- OpenVR driver: `9FA2B70005D4E0AEA02A891AE48A5CAA0314D0F30E1190001D7EA979EC10D5C1`.

The **127-byte** installed `OpenVRDriver/driver.vrdrivermanifest` has SHA256 `447AA45C7AC251D6ECDAC8A36EDF888B67AA945E47B3CB8C52D2CA7E64D469B8`. Its literal declarations are `name: VirtualDesktop`, `alwaysActivate: true`, `redirectsDisplay: true`, `hmd_presence: ["*.*"]`. These and the factory export establish the presence of an OpenVR driver artifact. They do not establish registration, successful loading, device creation or a shared-memory layout.

## Limits and implications

This is **static inventory and resource/header decoding**, not decompilation, disassembly of function bodies, or runtime observation. REA MCP/CLI and decompilation engines were unavailable; none were installed. No target assembly was loaded, target executable run, APK installed, or service/configuration changed. Only existing trusted `aapt.exe` and `objdump.exe` were executed, plus PowerShell/.NET file and ZIP APIs. No behavior is inferred from arbitrary strings. Export names establish symbols, not signatures or behavior.

The local objdump cannot recognize three ARM64 driver files (`AudioDriver/Arm64/vdvad.sys`, `GamepadDriver/Arm64/vdvge.sys`, `MonitorDriver/Arm64/vdvmd.dll`). Their SHA256, size, ARM64 PE machine value and CLR-directory observations are still recorded; export coverage is unavailable, not zero. x64 kernel-driver section warnings are retained as diagnostics. Successful named-export matching does not inventory ordinal-only exports or managed methods.

The collector caps PE files at 256, APK selected members at 256, per-PE export samples at 48 rows plus 32 relevant symbol rows, manifest text at 700 lines/90,000 characters, and badging at 160 lines/30,000 characters. JSON is rejected above 1.5 million characters. Truncation flags and total match counts distinguish omitted samples from absent data. This run covers every matching PE file and APK member; some large export tables are intentionally sampled. Nonzero tool exit codes/diagnostics remain attached to their artifacts; top-level errors describe collector failures separately.

The inventory supports a Windows CLI/application plus native OpenVR/OpenXR artifact boundary and an Android DEX/native/Mono packaging boundary. It **does not recover Virtual Desktop's IPC offsets, tracking serialization, network protocol, handshake, service authorization, texture transport, or timing contracts**. Those require actual managed/native code analysis or explicitly authorized runtime evidence. Nothing here validates Prism interoperability or full PCVR.

## Reproduction and validation

Run `& ./tools/inspect_reference_artifacts.ps1` from PowerShell; override `-ApkPath`, `-InstallRoots`, `-AaptPath`, `-ObjdumpPath` or `-OutputPath` when needed. Outputs are written only to the specified JSON path; APK and installation paths are opened for reading. Hashing native ZIP members streams decompressed bytes in memory without writing extracted binaries.

The collector completed with 36 PE records, one driver manifest and zero top-level collection errors. Three unsupported-ARM64 objdump exit codes remain explicitly recorded. PowerShell parser validation passed. JSON validation checked 243 SHA256/size identities, the 36-file PE count, complete 205-member APK coverage (203 native libraries), successful and untruncated aapt manifest output, and the OpenVR factory export. The evidence file is 228,337 bytes. Hashes identify bytes, not publisher signatures or trust, and do not prove the installation stayed unchanged for the entire collection interval.

