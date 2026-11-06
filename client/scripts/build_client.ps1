<# Builds offline. Falls back to installed SDK tools when Gradle is absent. #>
[CmdletBinding()]
param([ValidateSet('Debug','Release')][string]$Configuration = 'Release', [switch]$Standalone)
$ErrorActionPreference = 'Stop'
$ClientDir = Split-Path -Parent $PSScriptRoot
$Gradle = Join-Path $ClientDir 'gradlew.bat'
if (-not (Test-Path $Gradle)) {
    $command = Get-Command gradle -ErrorAction SilentlyContinue
    $Gradle = if ($command) { $command.Source } else { $null }
}
if ($Standalone -or -not $Gradle) {
    & "$PSScriptRoot/package_apk.ps1" -Configuration $Configuration
    return
}
Push-Location $ClientDir
try {
    & $Gradle --offline "assemble$Configuration"
    if ($LASTEXITCODE -ne 0) { throw "Offline Gradle failed ($LASTEXITCODE). Use -Standalone for the local SDK build." }
    $apk = Join-Path $ClientDir "app/build/outputs/apk/$($Configuration.ToLower())/app-$($Configuration.ToLower()).apk"
    if (-not (Test-Path $apk)) { throw "Gradle did not produce $apk" }
    Copy-Item $apk "$ClientDir/PrismAperture.apk" -Force
    Copy-Item $apk "$ClientDir/OpenRealityClient.apk" -Force
} finally { Pop-Location }
