<# Offline standalone Android build. Uses only project sources and installed tools. #>
[CmdletBinding()]
param(
    [ValidateSet('Debug','Release')][string]$Configuration = 'Release',
    [string]$SdkRoot = '',
    [string]$JavaHome = '',
    [switch]$Unsigned,
    [switch]$NativeOnly,
    [switch]$CheckTools
)
$ErrorActionPreference = 'Stop'
$ClientDir = Split-Path -Parent $PSScriptRoot
function Require-File([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "Required local tool/input missing: $Path. Install it separately; this script never downloads dependencies." }
    return $Path
}
function Run([string]$Tool, [string[]]$Arguments) {
    & $Tool @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Tool failed with exit code $LASTEXITCODE" }
}
function Latest-Directory([string]$Path) {
    $item = Get-ChildItem -LiteralPath $Path -Directory | Sort-Object { try { [version]($_.Name -replace '^android-','') } catch { [version]'0.0' } } -Descending | Select-Object -First 1
    if (-not $item) { throw "No installed versions in $Path" }
    return $item.FullName
}
if (-not $SdkRoot) {
    foreach ($candidate in @($env:ANDROID_SDK_ROOT, $env:ANDROID_HOME, (Join-Path $ClientDir 'tools/android-sdk'), "$env:LOCALAPPDATA/Android/Sdk")) {
        if ($candidate -and (Test-Path -LiteralPath "$candidate/build-tools")) { $SdkRoot = $candidate; break }
    }
}
if (-not $SdkRoot) { throw 'No local Android SDK found. Specify -SdkRoot.' }
$SdkRoot = (Resolve-Path -LiteralPath $SdkRoot).Path
if (-not $JavaHome) {
    if ($env:JAVA_HOME -and (Test-Path "$env:JAVA_HOME/bin/javac.exe")) { $JavaHome = $env:JAVA_HOME }
    else { $JavaHome = Split-Path -Parent (Split-Path -Parent (Get-Command javac.exe -ErrorAction Stop).Source) }
}
$Javac = Require-File "$JavaHome/bin/javac.exe"
$Java = Require-File "$JavaHome/bin/java.exe"
$Keytool = Require-File "$JavaHome/bin/keytool.exe"
$Jar = Require-File "$JavaHome/bin/jar.exe"
$BuildTools = Latest-Directory "$SdkRoot/build-tools"
$AndroidJar = Require-File "$(Latest-Directory "$SdkRoot/platforms")/android.jar"
$Ndk = Latest-Directory "$SdkRoot/ndk"
$CmakeBin = "$(Latest-Directory "$SdkRoot/cmake")/bin"
$Cmake = Require-File "$CmakeBin/cmake.exe"
$Ninja = Require-File "$CmakeBin/ninja.exe"
$Aapt = Require-File "$BuildTools/aapt2.exe"
$Align = Require-File "$BuildTools/zipalign.exe"
$D8 = Require-File "$BuildTools/lib/d8.jar"
$Signer = Require-File "$BuildTools/lib/apksigner.jar"
$Main = Join-Path $ClientDir 'app/src/main'
$Loader = Require-File "$Main/jniLibs/arm64-v8a/libopenxr_loader.so"
$Stl = Require-File "$Ndk/toolchains/llvm/prebuilt/windows-x86_64/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
Write-Host "Offline SDK: $SdkRoot; platform: $AndroidJar; JDK: $JavaHome; NDK: $Ndk"
Write-Host 'OpenXR loader is an explicit project dependency; no APK or extracted resources are used.'
if ($CheckTools) { return }
# Unique workspace avoids stale outputs, shared native build trees, and recursive deletion.
$Work = Join-Path $ClientDir ("app/build/standalone/" + [guid]::NewGuid().ToString('N'))
foreach ($dir in @($Work,"$Work/gen","$Work/classes","$Work/dex","$Work/native","$Work/payload/lib/arm64-v8a")) {
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
}
$BuildType = if ($Configuration -eq 'Debug') { 'Debug' } else { 'Release' }
Run $Cmake @('-S',"$Main/cpp",'-B',"$Work/native",'-G','Ninja',"-DCMAKE_MAKE_PROGRAM=$Ninja","-DCMAKE_TOOLCHAIN_FILE=$Ndk/build/cmake/android.toolchain.cmake",'-DANDROID_ABI=arm64-v8a','-DANDROID_PLATFORM=android-29','-DANDROID_STL=c++_shared',"-DCMAKE_BUILD_TYPE=$BuildType")
Run $Cmake @('--build',"$Work/native",'--target','prism_aperture_client','--parallel','2')
if ($NativeOnly) {
    Copy-Item "$Work/native/libprism_aperture_client.so" "$Main/jniLibs/arm64-v8a/" -Force
    Copy-Item $Stl "$Main/jniLibs/arm64-v8a/" -Force
    Write-Host "Native build complete: $Work/native/libprism_aperture_client.so"
    return
}
Run $Aapt @('compile','--dir',"$Main/res",'-o',"$Work/resources.zip")
# Link the actual manifest, retaining its SDK and version declarations.
$linkArgs = @('link','-o',"$Work/resources.apk",'-I',$AndroidJar,'--manifest',"$Main/AndroidManifest.xml",'--java',"$Work/gen",'--auto-add-overlay','-0','arsc')
if (Test-Path "$Main/assets") { $linkArgs += @('-A',"$Main/assets") }
$linkArgs += "$Work/resources.zip"
Run $Aapt $linkArgs
$Sources = @(Get-ChildItem "$Main/java","$Work/gen" -Filter '*.java' -Recurse | ForEach-Object { '"' + $_.FullName.Replace('\','/') + '"' })
$Sources | Set-Content "$Work/sources.txt" -Encoding ASCII
Run $Javac @('-encoding','UTF-8','-source','17','-target','17','-classpath',$AndroidJar,'-d',"$Work/classes","@$Work/sources.txt")
Run $Jar @('--create','--file',"$Work/classes.jar",'-C',"$Work/classes",'.')
Run $Java @('-cp',$D8,'com.android.tools.r8.D8','--lib',$AndroidJar,'--min-api','29','--output',"$Work/dex","$Work/classes.jar")
Copy-Item "$Work/native/libprism_aperture_client.so" "$Work/payload/lib/arm64-v8a/"
Copy-Item $Loader "$Work/payload/lib/arm64-v8a/"
Copy-Item $Stl "$Work/payload/lib/arm64-v8a/"
Copy-Item "$Work/dex/*.dex" "$Work/payload/"
Copy-Item "$Work/resources.apk" "$Work/unaligned.apk"
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [System.IO.Compression.ZipFile]::Open("$Work/unaligned.apk", [System.IO.Compression.ZipArchiveMode]::Update)
try {
    foreach ($file in Get-ChildItem "$Work/payload" -File -Recurse) {
        $entry = $file.FullName.Substring(("$Work/payload").Length + 1).Replace('\','/')
        [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip,$file.FullName,$entry,[System.IO.Compression.CompressionLevel]::NoCompression) | Out-Null
    }
} finally { $zip.Dispose() }
Run $Align @('-p','-f','4',"$Work/unaligned.apk","$Work/aligned.apk")
Run $Align @('-c','-p','4',"$Work/aligned.apk")
if ($Unsigned) { Write-Host "Unsigned APK: $Work/aligned.apk"; return }
# Persistent, local debug identity for sideloading, never a production signing key.
$KeyStore = Join-Path $ClientDir 'debug.keystore'
if (-not (Test-Path -LiteralPath $KeyStore)) { $KeyStore = Join-Path $ClientDir 'app/build/standalone/debug.keystore' }
if (-not (Test-Path -LiteralPath $KeyStore)) {
    Run $Keytool @('-genkeypair','-keystore',$KeyStore,'-storepass','android','-keypass','android','-alias','androiddebugkey','-dname','CN=Android Debug,O=Android,C=US','-keyalg','RSA','-validity','10000','-noprompt')
}
Run $Java @('-jar',$Signer,'sign','--ks',$KeyStore,'--ks-pass','pass:android','--key-pass','pass:android','--ks-key-alias','androiddebugkey','--out',"$Work/signed.apk","$Work/aligned.apk")
Run $Java @('-jar',$Signer,'verify','--verbose',"$Work/signed.apk")
Copy-Item "$Work/signed.apk" "$ClientDir/PrismAperture.apk" -Force
Copy-Item "$Work/signed.apk" "$ClientDir/OpenRealityClient.apk" -Force
Write-Host "Signed standalone APK: $ClientDir/PrismAperture.apk (debug signing identity)"



