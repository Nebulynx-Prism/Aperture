[CmdletBinding()]
param(
    [string]$ApkPath = 'C:\Rookie\Virtual Desktop v10708+1.34.22.0 -ZuRiUs\VirtualDesktop.Android.apk',
    [string[]]$InstallRoots = @('C:\Program Files\Virtual Desktop Streamer', 'C:\Program Files\Virtual Desktop'),
    [string]$AaptPath = 'C:\Rookie\platform-tools\aapt.exe',
    [string]$ObjdumpPath = (Join-Path $PSScriptRoot '../w64devkit/bin/objdump.exe'),
    [string]$OutputPath = (Join-Path $PSScriptRoot '../docs/reference-artifacts.json')
)
# Static reads only. Never load assemblies or invoke inspected executables.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem
function Get-Identity([string]$Path) {
    $item = Get-Item -LiteralPath $Path
    [ordered]@{ path = $item.FullName; size = $item.Length; sha256 = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash }
}
function Get-PeHeader([string]$Path) {
    $stream = [IO.File]::OpenRead($Path)
    $reader = [IO.BinaryReader]::new($stream)
    try {
        if ($reader.ReadUInt16() -ne 0x5a4d) { return @{ validPe = $false } }
        $stream.Position = 0x3c
        $peOffset = $reader.ReadUInt32()
        if ($peOffset + 24 -gt $stream.Length) { throw 'PE offset outside file' }
        $stream.Position = $peOffset
        if ($reader.ReadUInt32() -ne 0x4550) { return @{ validPe = $false } }
        $machine = $reader.ReadUInt16()
        $stream.Position = $peOffset + 20
        $optionalSize = $reader.ReadUInt16()
        $optional = $peOffset + 24
        $stream.Position = $optional
        $magic = $reader.ReadUInt16()
        $directoryOffset = switch ($magic) { 0x10b { 96 } 0x20b { 112 } default { throw 'Unknown optional header' } }
        $stream.Position = $optional + $directoryOffset - 4
        $directoryCount = $reader.ReadUInt32()
        $clrRva = 0; $clrSize = 0
        if ($directoryCount -gt 14 -and $optionalSize -ge ($directoryOffset + 120)) {
            $stream.Position = $optional + $directoryOffset + 14 * 8
            $clrRva = $reader.ReadUInt32(); $clrSize = $reader.ReadUInt32()
        }
        return [ordered]@{ validPe = $true; machine = ('0x{0:X4}' -f $machine); optionalMagic = ('0x{0:X}' -f $magic); clrDirectoryRva = $clrRva; clrDirectorySize = $clrSize; hasClrDirectory = ($clrRva -ne 0 -and $clrSize -ne 0) }
    } finally { $reader.Dispose(); $stream.Dispose() }
}
function Invoke-StaticTool([string]$Tool, [string[]]$ToolArgs, [int]$MaxLines, [int]$MaxCharacters = 100000) {
    if (!(Test-Path -LiteralPath $Tool)) { return @{ available = $false; path = $Tool } }
    $lines = @(& $Tool @ToolArgs 2>&1 | ForEach-Object { "$_" })
    $exitCode = $LASTEXITCODE
    $selected = @($lines | Select-Object -First $MaxLines)
    $body = $selected -join "`n"
    $truncated = ($lines.Count -gt $MaxLines -or $body.Length -gt $MaxCharacters)
    if ($body.Length -gt $MaxCharacters) { $body = $body.Substring(0, $MaxCharacters) }
    return [ordered]@{ available = $true; path = (Get-Item -LiteralPath $Tool).FullName; args = $ToolArgs; exitCode = $exitCode; totalLines = $lines.Count; truncated = $truncated; output = $body }
}
$errors = [Collections.Generic.List[string]]::new()
$records = [Collections.Generic.List[object]]::new()
$manifests = [Collections.Generic.List[object]]::new()
foreach ($root in $InstallRoots) {
    if (!(Test-Path -LiteralPath $root)) { $errors.Add("Missing root: $root"); continue }
    $files = @(Get-ChildItem -LiteralPath $root -Recurse -File | Sort-Object FullName)
    foreach ($file in $files) {
        if ($records.Count -ge 256) { $errors.Add('PE inventory capped at 256 files'); break }
        if ($file.Extension -notin @('.exe', '.dll', '.sys')) { continue }
        try {
            $record = Get-Identity $file.FullName
            $record.pe = Get-PeHeader $file.FullName
            # Only export-name rows retained; imports/string literals are not interpreted.
            if (Test-Path -LiteralPath $ObjdumpPath) {
                                $savedPreference = $ErrorActionPreference
                try {
                    $ErrorActionPreference = 'Continue'
                    $dump = @(& $ObjdumpPath -p $file.FullName 2>&1 | ForEach-Object { "$_" })
                } finally { $ErrorActionPreference = $savedPreference }
                $code = $LASTEXITCODE
                $names = @($dump | Where-Object { $_ -match '^\s*\[\s*\d+\]\s+(?:\+base\[\s*\d+\]\s+[0-9a-fA-F]+\s+)?[^\s]+\s*$' })
                $record.exports = @{ exitCode = $code; matchedRows = $names.Count; truncated = ($names.Count -gt 48); rows = @($names | Select-Object -First 48); relevantRows = @($names | Where-Object { $_ -match 'HmdDriverFactory|xrNegotiate|xrGet|xrCreate|xrDestroy|WLAN' } | Select-Object -First 32); diagnostics = @($dump | Where-Object { $_ -match 'objdump.exe:|not recognized|warning:|error:' } | Select-Object -First 12) }
            }
            $records.Add($record)
        } catch { $errors.Add("$($file.FullName): $_") }
    }
    foreach ($file in @($files | Where-Object { $_.Name -eq 'driver.vrdrivermanifest' })) {
        $manifest = Get-Identity $file.FullName
        $manifest.content = if ($file.Length -le 32768) { [IO.File]::ReadAllText($file.FullName) } else { 'Omitted: exceeds 32768 bytes' }
        $manifests.Add($manifest)
    }
}
$apk = $null
if (Test-Path -LiteralPath $ApkPath) {
    $apk = Get-Identity $ApkPath
    $zip = [IO.Compression.ZipFile]::OpenRead($ApkPath)
    try {
        $members = @($zip.Entries | Where-Object { $_.FullName -match '(^classes\d*\.dex$|^lib/.+\.so$|^AndroidManifest.xml$)' } | Sort-Object FullName)
        $inventory = @($members | Select-Object -First 256 | ForEach-Object {
            $entryStream = $_.Open(); $hasher = [Security.Cryptography.SHA256]::Create()
            try { $hash = [BitConverter]::ToString($hasher.ComputeHash($entryStream)).Replace('-', '') }
            finally { $entryStream.Dispose(); $hasher.Dispose() }
            [ordered]@{ name = $_.FullName; size = $_.Length; compressedSize = $_.CompressedLength; sha256 = $hash }
        })
        $apk.entryCount = $zip.Entries.Count
        $apk.selectedEntryCount = $members.Count
        $apk.inventoryTruncated = ($members.Count -gt 256)
        $apk.inventory = $inventory
    } finally { $zip.Dispose() }
    $apk.badging = Invoke-StaticTool $AaptPath @('dump', 'badging', $ApkPath) 160 30000
    $apk.manifest = Invoke-StaticTool $AaptPath @('dump', 'xmltree', $ApkPath, 'AndroidManifest.xml') 700 90000
} else { $errors.Add("Missing APK: $ApkPath") }
$evidence = [ordered]@{
    schemaVersion = 1; generatedUtc = [DateTime]::UtcNow.ToString('o')
    method = 'Read-only file hashes, PE header parsing, objdump export inventory, ZIP inventory and aapt resource decoding. No target execution, assembly loading, decompilation or runtime observation.'
    roots = $InstallRoots; bounds = @{ maxPeFiles = 256; maxExportRowsPerFile = 48; maxApkMembers = 256; maxBadgingLines = 160; maxManifestLines = 700 }
    tools = @{ aapt = $(if (Test-Path -LiteralPath $AaptPath) { Get-Identity $AaptPath }); objdump = $(if (Test-Path -LiteralPath $ObjdumpPath) { Get-Identity $ObjdumpPath }) }
    peFiles = @($records.ToArray()); driverManifests = @($manifests.ToArray()); apk = $apk; errors = @($errors.ToArray())
}
$json = $evidence | ConvertTo-Json -Depth 16
if ($json.Length -gt 1500000) { throw 'Evidence exceeds 1.5 million character output bound' }
[IO.File]::WriteAllText([IO.Path]::GetFullPath($OutputPath), $json, [Text.UTF8Encoding]::new($false))
Write-Output "Saved $OutputPath; PE files=$($records.Count); manifests=$($manifests.Count); errors=$($errors.Count)"


