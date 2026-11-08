[CmdletBinding()]
param([switch]$SkipTests)
$ErrorActionPreference='Stop'
$previous=Get-Location
try {
    Set-Location $PSScriptRoot
    $ErrorActionPreference='Continue'
    if(!$SkipTests){& cargo test --offline --target x86_64-pc-windows-msvc;if($LASTEXITCODE -ne 0){throw 'Rust tests failed'}}
    & cargo build --offline --release --target x86_64-pc-windows-msvc
    if($LASTEXITCODE -ne 0){throw 'Rust core build failed'}
    $ErrorActionPreference='Stop'
    $vswhere="${env:ProgramFiles(x86)}/Microsoft Visual Studio/Installer/vswhere.exe"
    $msbuild=@(& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -find 'MSBuild/**/Bin/MSBuild.exe')[0]
    if(!$msbuild){throw 'MSVC is required for Valve C++ ABI compatibility'}
    & $msbuild (Join-Path $PSScriptRoot 'abi/aperture.vcxproj') /m /p:Configuration=Release /p:Platform=x64 /verbosity:minimal
    if($LASTEXITCODE -ne 0){throw 'OpenVR ABI shim build failed'}
    Write-Output 'Built staged Aperture-owned Rust driver. No registration or runtime switch performed.'
} finally {Set-Location $previous}
