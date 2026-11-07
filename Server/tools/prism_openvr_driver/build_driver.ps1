[CmdletBinding()]
param([string]$OutputDirectory='')
$ErrorActionPreference='Stop'
# OpenVR exposes C++ virtual callbacks returning structures by value. MinGW's
# C++ ABI is incompatible with the MSVC SteamVR host even on Windows x64.
$vswhere="${env:ProgramFiles(x86)}/Microsoft Visual Studio/Installer/vswhere.exe"
if(!(Test-Path -LiteralPath $vswhere)) { throw 'MSVC C++ Build Tools are required for the SteamVR driver ABI.' }
$msbuild=@(& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -find 'MSBuild/**/Bin/MSBuild.exe')[0]
if(!$msbuild) { throw 'MSVC C++ Build Tools are required; do not substitute MinGW for this driver.' }
$installation=& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
$toolset=(Get-ChildItem "$installation/MSBuild/Microsoft/VC/*/Platforms/x64/PlatformToolsets/*" -Directory | Sort-Object Name -Descending | Select-Object -First 1).Name
if(!$toolset) { throw 'No installed MSVC platform toolset found.' }
$out=if($OutputDirectory){[IO.Path]::GetFullPath($OutputDirectory)}else{Join-Path $PSScriptRoot 'driver/bin/win64'}
New-Item -ItemType Directory -Force $out | Out-Null
$library=Join-Path $out 'driver_prism_aperture.dll'
$backup=Join-Path $PSScriptRoot 'target/driver_before_msvc.dll'
New-Item -ItemType Directory -Force (Split-Path $backup -Parent) | Out-Null
if((Test-Path -LiteralPath $library) -and !(Test-Path -LiteralPath $backup)) { Copy-Item -LiteralPath $library -Destination $backup }
& $msbuild (Join-Path $PSScriptRoot 'native_msvc.vcxproj') /m /p:Configuration=Release /p:Platform=x64 "/p:PlatformToolset=$toolset" /p:TargetName=driver_prism_aperture "/p:OutDir=$out/" "/p:IntDir=$PSScriptRoot/target/driver_obj/" /verbosity:minimal
if($LASTEXITCODE -ne 0) { throw 'MSVC driver compilation failed' }
Write-Output "Built MSVC-compatible OpenVR driver: $library"
