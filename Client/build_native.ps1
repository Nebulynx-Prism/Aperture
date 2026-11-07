<# Build current native sources with installed SDK tools, without deleting shared directories. #>
[CmdletBinding()]
param(
    [ValidateSet('Debug','Release')][string]$Configuration = 'Release',
    [string]$SdkRoot = '',
    [string]$JavaHome = ''
)
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot/scripts/package_apk.ps1" @PSBoundParameters -NativeOnly
