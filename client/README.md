# Prism Aperture Client for Meta Quest

Prism Aperture is an experimental Android/OpenXR desktop streaming client for
Quest headsets. The source includes device profiles, local server discovery,
MediaCodec video decoding, audio playback, and controller input. Successful
compilation does not establish headset compatibility or end-to-end streaming.
PCVR/SteamVR game streaming, performance, hand tracking, and passthrough support
have not been verified end to end.

- Package: `org.nebulynx.aperture`
- Android sources: `app/src/main/` (Java, C++, manifest, resources, assets)
- Native dependencies: project OpenXR headers/loader and the installed NDK runtime

## Build offline

Requires a local JDK 17, Android SDK platform, build-tools, NDK, and CMake/Ninja.
The scripts discover the SDK from `ANDROID_SDK_ROOT`, `ANDROID_HOME`, or the
repository's `tools/android-sdk`. No dependencies are downloaded.

From `client/`:

```powershell
.\scripts\package_apk.ps1 -CheckTools
.\scripts\package_apk.ps1
```

This compiles current native sources first, then resources and Java, runs D8,
aligns the APK, signs with the local debug keystore, and verifies the signature.
The outputs are `PrismAperture.apk` and the alias `OpenRealityClient.apk`.
Use `-Unsigned` to stop before signing, or `-SdkRoot` and `-JavaHome` to select tools.
`build_custom_apk.ps1` delegates to the same pipeline. `build_native.ps1` builds
only native libraries. Each build uses a separate directory under
`app/build/standalone/`; existing build directories are not recursively deleted.
No external APK is used as a packaging template.

`./scripts/build_client.ps1` uses Gradle offline if available, otherwise the
standalone pipeline. Pass `-Standalone` to select the latter explicitly.

## Install separately

After reviewing the final build, `scripts/install_quest.ps1` can install and
launch it through ADB. Building does not install or launch anything on a headset.
