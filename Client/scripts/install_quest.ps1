<#
.SYNOPSIS
    Sideload and launch Prism Aperture on a connected Meta Quest headset.
    Package ID: org.nebulynx.aperture
#>

[CmdletBinding()]
param(
    [string]$ApkPath = ""
)

$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ClientDir = Split-Path -Parent $ScriptDir

# Locate adb
$AdbPaths = @(
    "C:\Rookie\platform-tools\adb.exe",
    "C:\Program Files (x86)\ADB & Fastboot++\adb.exe",
    "C:\Program Files\Android\platform-tools\adb.exe",
    "$env:LOCALAPPDATA\Android\Sdk\platform-tools\adb.exe",
    "adb.exe"
)

$Adb = $null
foreach ($p in $AdbPaths) {
    if (Test-Path $p) {
        $Adb = $p
        break
    }
}

if (-not $Adb) {
    $found = Get-Command "adb" -ErrorAction SilentlyContinue
    if ($found) { $Adb = $found.Source }
}

if (-not $Adb) {
    Write-Error "ADB executable not found. Please ensure ADB is installed and added to PATH."
    return
}

Write-Host "Using ADB: $Adb" -ForegroundColor Cyan

# Locate APK
if ([string]::IsNullOrEmpty($ApkPath)) {
    $PossibleApks = @(
        (Join-Path $ClientDir "PrismAperture.apk"),
        (Join-Path $ClientDir "OpenRealityClient.apk"),
        (Join-Path $ClientDir "app/build/outputs/apk/release/app-release.apk"),
        (Join-Path $ClientDir "app/build/outputs/apk/debug/app-debug.apk")
    )
    foreach ($cand in $PossibleApks) {
        if (Test-Path $cand) {
            $ApkPath = $cand
            break
        }
    }
}

if (-not (Test-Path $ApkPath)) {
    Write-Error "Prism Aperture APK not found at: $ApkPath. Please run build_client.ps1 first."
    return
}

Write-Host "Target APK: $ApkPath" -ForegroundColor Cyan

# Check connected devices
Write-Host "Querying connected VR devices..." -ForegroundColor Yellow
$devices = & $Adb devices | Where-Object { $_ -match "\bdevice\b" -and $_ -notmatch "List of devices" }

if (-not $devices) {
    Write-Error "No connected Quest devices found. Please plug in your Quest 2, Quest 3, or Quest 3S via USB-C or Wi-Fi and accept the USB debugging prompt in VR."
    return
}

Write-Host "Connected Devices:" -ForegroundColor Green
$devices | ForEach-Object { Write-Host "  $_" -ForegroundColor Green }

# Query device model
$deviceModel = (& $Adb shell getprop ro.product.model).Trim()
$deviceSoc = (& $Adb shell getprop ro.board.platform).Trim()
Write-Host "Detected Headset: $deviceModel (Platform: $deviceSoc)" -ForegroundColor Magenta

# Sideload APK
Write-Host "Installing Prism Aperture with runtime permissions..." -ForegroundColor Yellow
& $Adb install -r -g $ApkPath

if ($LASTEXITCODE -ne 0) {
    Write-Error "ADB install failed with code $LASTEXITCODE"
    return
}

Write-Host "Installation successful!" -ForegroundColor Green

# Launch the app in Quest VR mode
Write-Host "Launching Prism Aperture on $deviceModel..." -ForegroundColor Cyan
& $Adb shell monkey -p org.nebulynx.aperture -c android.intent.category.LAUNCHER 1 | Out-Null
if ($LASTEXITCODE -ne 0) {
    & $Adb shell am start -n "org.nebulynx.aperture/.MainActivity" -a android.intent.action.MAIN -c com.oculus.intent.category.VR
}

Write-Host "==========================================================" -ForegroundColor Green
Write-Host " Prism Aperture is now running on your Quest!" -ForegroundColor Green
Write-Host " It will automatically search for your Prism Aperture Server" -ForegroundColor Green
Write-Host " via local mDNS/UDP broadcast on port 38810." -ForegroundColor Green
Write-Host "==========================================================" -ForegroundColor Green
