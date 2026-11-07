<# Compatibility entry point. The standalone pipeline builds current native and Java sources. #>
[CmdletBinding()]
param(
    [ValidateSet('Debug','Release')][string]$Configuration = 'Release',
    [string]$SdkRoot = '',
    [string]$JavaHome = '',
    [switch]$Unsigned,
    [switch]$CheckTools
)
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot/scripts/package_apk.ps1" @PSBoundParameters
