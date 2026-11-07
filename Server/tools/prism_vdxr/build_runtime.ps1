[CmdletBinding()]
param([switch]$PrepareOnly)
$ErrorActionPreference='Stop'
& (Join-Path $PSScriptRoot 'prepare_upstream.ps1')
if($PrepareOnly) { return }
$vswhere="${env:ProgramFiles(x86)}/Microsoft Visual Studio/Installer/vswhere.exe"
if(!(Test-Path -LiteralPath $vswhere)) { throw 'Microsoft C++ Build Tools are required; none were found.' }
$msbuild=& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -find 'MSBuild/**/Bin/MSBuild.exe'
if(!$msbuild) { throw 'No completed Visual Studio C++ installation found. Backend tests can still run with test_backend.ps1.' }
$msbuild=@($msbuild)[0]
$installation=& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
$toolsets=Get-ChildItem "$installation/MSBuild/Microsoft/VC/*/Platforms/x64/PlatformToolsets/*" -Directory
$toolset=($toolsets | Sort-Object Name -Descending | Select-Object -First 1).Name
if(!$toolset) { throw 'No installed MSVC platform toolset found.' }
Write-Output "Building with installed MSVC toolset $toolset"
$checkout=Join-Path $PSScriptRoot 'upstream'
$bin=Join-Path $PSScriptRoot 'bin'
$obj=Join-Path $PSScriptRoot 'obj'
New-Item -ItemType Directory -Force $bin,$obj | Out-Null
# Preserve upstream licensing in the adapted runtime distribution.
Copy-Item -LiteralPath (Join-Path $checkout 'LICENSE') -Destination (Join-Path $bin 'LICENSE-VDXR.txt')
[IO.File]::WriteAllText((Join-Path $obj 'commit.h'),'const char* RuntimeCommitHash = "6da20fe-prism";')
& $msbuild (Join-Path $checkout 'virtualdesktop-openxr/prism-vdxr.vcxproj') /m:2 /p:Configuration=Release /p:Platform=x64 "/p:PlatformToolset=$toolset" "/p:SolutionDir=$checkout\" "/p:OutDir=$bin\" "/p:IntDir=$obj\" /p:TargetName=prism-vdxr /p:PreBuildEventUseInBuild=false /p:PostBuildEventUseInBuild=false
if($LASTEXITCODE -ne 0) { throw "VDXR compilation failed ($LASTEXITCODE)" }
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'prism-vdxr.json') -Destination $bin
Write-Output "Built $bin/prism-vdxr.dll (not globally registered)"
