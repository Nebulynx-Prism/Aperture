[CmdletBinding()]
param([switch]$SkipBuild)
$ErrorActionPreference='Stop'
$server=Split-Path $PSScriptRoot
Push-Location $server
try {
    if(!$SkipBuild) {
        & ./build.ps1
        if($LASTEXITCODE -ne 0){throw 'Server build failed'}
    }
    $payload=Join-Path $server 'target/installer-payload'
    $cache=Join-Path $server 'target/installer-cache'
    New-Item -ItemType Directory -Force $payload,$cache | Out-Null
    $archive=Join-Path $cache 'ffmpeg.zip'
    $expected='60F467265B1E312373DBCD92200C2618A74850F98D3D078E94296BB3FA2047BA'
    if(!(Test-Path -LiteralPath $archive)) {
        [Net.ServicePointManager]::SecurityProtocol=[Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -UseBasicParsing 'https://www.gyan.dev/ffmpeg/builds/packages/ffmpeg-9.0.2-essentials_build.zip' -OutFile $archive -TimeoutSec 180
    }
    if((Get-FileHash -LiteralPath $archive).Hash -ne $expected){throw 'FFmpeg archive SHA256 mismatch'}
    $expanded=Join-Path $cache 'ffmpeg'
    if(!(Test-Path -LiteralPath (Join-Path $expanded 'ffmpeg-9.0.2-essentials_build/bin/ffmpeg.exe'))) {
        Expand-Archive -LiteralPath $archive -DestinationPath $expanded -Force
    }
    $distribution=Join-Path $expanded 'ffmpeg-9.0.2-essentials_build'
    Copy-Item -LiteralPath (Join-Path $distribution 'bin/ffmpeg.exe') -Destination $payload -Force
    $license=Join-Path $payload 'ffmpeg-license'
    New-Item -ItemType Directory -Force $license | Out-Null
    Get-ChildItem -LiteralPath $distribution -File | ForEach-Object {Copy-Item -LiteralPath $_.FullName -Destination $license -Force}
    & ./w64devkit/bin/makensis.exe /V3 installer.nsi
    if($LASTEXITCODE -ne 0){throw 'NSIS compilation failed'}
    Get-Item -LiteralPath 'target/release/Aperture-Setup.exe' | Select-Object FullName,Length
    $hash=Get-FileHash -LiteralPath 'target/release/Aperture-Setup.exe'
    [IO.File]::WriteAllText((Join-Path $server 'target/release/Aperture-Setup.exe.sha256'), ($hash.Hash.ToLowerInvariant()+'  Aperture-Setup.exe'+"`n"))
    Write-Output ("SHA256: " + $hash.Hash)
} finally {Pop-Location}


