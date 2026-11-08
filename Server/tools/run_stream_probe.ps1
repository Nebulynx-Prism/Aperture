<#
Offline standalone probe build; defaults to BUILD ONLY (no ADB/device access).
Examples after the parent starts the bounded server and provisions USB reverse:
  ./tools/run_stream_probe.ps1 -Run -ServerHost 127.0.0.1 -Transport Usb
  ./tools/run_stream_probe.ps1 -Run -ServerHost 192.168.1.20 -Transport Wifi -SamplePixels
Requires only an installed JDK and Android SDK build-tools/platform android.jar.
Does not install/alter any APK, change device settings, start the server, create
ADB port mappings, or store desktop images. -Run pushes only the diagnostic JAR
to /data/local/tmp/prism-stream-probe.jar. Wi-Fi here means LAN transport to the
server; ADB can remain connected over USB. This is not a headset-rendering test.
#>
[CmdletBinding()]
param(
    [switch]$Run,
    [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9.:-]{0,252}$')][string]$ServerHost = '127.0.0.1',
    [ValidateRange(1,30)][int]$DurationSeconds = 10,
    [ValidateRange(1,65535)][int]$Port = 38810,
    [ValidateSet('Usb','Wifi')][string]$Transport = 'Usb',
    [switch]$SamplePixels,
    [string]$SdkRoot = '',
    [string]$JavaHome = '',
    [string]$AdbPath = '',
    [ValidatePattern('^[A-Za-z0-9_.:-]+$')][string]$Serial = ''
)
$ErrorActionPreference = 'Stop'
$RootDir = Split-Path -Parent $PSScriptRoot
function Require-File([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Missing local input/tool: $Path. No dependencies will be downloaded."
    }
    return (Resolve-Path -LiteralPath $Path).Path
}
function Latest-Directory([string]$Path) {
    $dir = Get-ChildItem -LiteralPath $Path -Directory | Sort-Object {
        try { [version]($_.Name -replace '^android-','') } catch { [version]'0.0' }
    } -Descending | Select-Object -First 1
    if (-not $dir) { throw "No installed SDK versions in $Path" }
    return $dir.FullName
}
# Windows argv quoting (ProcessStartInfo.ArgumentList is absent in Windows PS5).
function Quote-Argument([string]$Value) {
    if ($Value.Length -gt 0 -and $Value -notmatch '[\s"]') { return $Value }
    $escaped = [regex]::Replace($Value, '(\\*)"', '$1$1\"')
    $escaped = [regex]::Replace($escaped, '(\\+)$', '$1$1')
    return '"' + $escaped + '"'
}
function Run-Bounded([string]$Tool, [string[]]$Arguments, [int]$TimeoutSeconds = 60) {
    $start = New-Object System.Diagnostics.ProcessStartInfo
    $start.FileName = $Tool
    $start.Arguments = (($Arguments | ForEach-Object { Quote-Argument $_ }) -join ' ')
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $start
    try {
        if (-not $process.Start()) { throw "Failed to start $Tool" }
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
            $process.Kill()
            $null = $process.WaitForExit(2000)
            throw "$Tool exceeded its ${TimeoutSeconds}s deadline. Remote probe also has its own hard watchdog."
        }
        $outText = $stdout.GetAwaiter().GetResult()
        $errText = $stderr.GetAwaiter().GetResult()
        if ($outText) { Write-Host $outText.TrimEnd() }
        if ($errText) { Write-Host $errText.TrimEnd() }
        if ($process.ExitCode -ne 0) { throw "$Tool exited with code $($process.ExitCode)" }
    } finally { $process.Dispose() }
}
if (-not $SdkRoot) {
    foreach ($candidate in @($env:ANDROID_SDK_ROOT, $env:ANDROID_HOME,
            (Join-Path $RootDir 'tools/android-sdk'), "$env:LOCALAPPDATA/Android/Sdk")) {
        if ($candidate -and (Test-Path -LiteralPath "$candidate/build-tools")) { $SdkRoot = $candidate; break }
    }
}
if (-not $SdkRoot) { throw 'No local Android SDK found; specify -SdkRoot.' }
$SdkRoot = (Resolve-Path -LiteralPath $SdkRoot).Path
if (-not $JavaHome) {
    if ($env:JAVA_HOME -and (Test-Path -LiteralPath "$env:JAVA_HOME/bin/javac.exe")) { $JavaHome = $env:JAVA_HOME }
    else { $JavaHome = Split-Path -Parent (Split-Path -Parent (Get-Command javac.exe -ErrorAction Stop).Source) }
}
$Javac = Require-File "$JavaHome/bin/javac.exe"
$Java = Require-File "$JavaHome/bin/java.exe"
$Jar = Require-File "$JavaHome/bin/jar.exe"
$AndroidJar = Require-File "$(Latest-Directory "$SdkRoot/platforms")/android.jar"
$D8 = Require-File "$(Latest-Directory "$SdkRoot/build-tools")/lib/d8.jar"
$Source = Require-File "$RootDir/client/tests/StreamProbe.java"
$Work = Join-Path $RootDir ("client/app/build/stream-probe/" + [guid]::NewGuid().ToString('N'))
foreach ($dir in @("$Work/classes", "$Work/dex")) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
Run-Bounded $Javac @('-encoding','UTF-8','-source','8','-target','8','-classpath',$AndroidJar,'-d',"$Work/classes",$Source)
Run-Bounded $Jar @('--create','--file',"$Work/classes.jar",'-C',"$Work/classes",'.')
# Pure protocol tests execute on the host; no emulator, server or device needed.
Run-Bounded $Java @('-cp',"$Work/classes.jar;$AndroidJar",'StreamProbe','--self-test') 10
Run-Bounded $Java @('-cp',$D8,'com.android.tools.r8.D8','--lib',$AndroidJar,'--min-api','29','--output',"$Work/dex","$Work/classes.jar")
$ProbeJar = "$Work/prism-stream-probe.jar"
Run-Bounded $Jar @('--create','--file',$ProbeJar,'-C',"$Work/dex",'classes.dex')
Write-Host "BUILD_OK $ProbeJar"
if (-not $Run) {
    Write-Host 'Build only: no device commands executed. Use -Run once the parent has prepared the server/transport.'
    return
}
if (-not $AdbPath) {
    if (Test-Path -LiteralPath "$SdkRoot/platform-tools/adb.exe") { $AdbPath = "$SdkRoot/platform-tools/adb.exe" }
    else { $AdbPath = (Get-Command adb.exe -ErrorAction Stop).Source }
}
$AdbPath = Require-File $AdbPath
$Device = @()
if ($Serial) { $Device = @('-s',$Serial) }
Run-Bounded $AdbPath ($Device + @('push',$ProbeJar,'/data/local/tmp/prism-stream-probe.jar')) 15
$sampleArg = if ($SamplePixels) { 'true' } else { 'false' }
# Every interpolated shell argument is restricted by ValidatePattern/Range/Set.
$Remote = "CLASSPATH=/data/local/tmp/prism-stream-probe.jar exec app_process /system/bin StreamProbe $ServerHost $DurationSeconds $Port $($Transport.ToLowerInvariant()) $sampleArg"
Run-Bounded $AdbPath ($Device + @('shell',$Remote)) ($DurationSeconds + 15)
