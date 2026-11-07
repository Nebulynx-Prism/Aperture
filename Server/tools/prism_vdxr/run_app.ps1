[CmdletBinding()]
param([Parameter(Mandatory)][string]$Application,[string[]]$ApplicationArguments=@())
$ErrorActionPreference='Stop'
$manifest=Join-Path $PSScriptRoot 'bin/prism-vdxr.json'
$library=Join-Path $PSScriptRoot 'bin/prism-vdxr.dll'
if(!(Test-Path -LiteralPath $manifest) -or !(Test-Path -LiteralPath $library)) { throw 'Build Prism VDXR with build_runtime.ps1 first.' }
$previous=$env:XR_RUNTIME_JSON
try {
    # Applies to this application and children only. No global registry change.
    $env:XR_RUNTIME_JSON=$manifest
    & $Application @ApplicationArguments
    if($LASTEXITCODE -ne 0) { throw "OpenXR application exited with $LASTEXITCODE" }
} finally { $env:XR_RUNTIME_JSON=$previous }
