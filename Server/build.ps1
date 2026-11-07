$ErrorActionPreference = 'Stop'
$mingwBin = Join-Path $PSScriptRoot 'w64devkit/bin'
if (Test-Path -LiteralPath (Join-Path $mingwBin 'gcc.exe')) {
    $env:PATH = "$mingwBin;$env:PATH"
}
Push-Location $PSScriptRoot
try {
    $previousErrorAction = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    cargo build --release --locked
    $buildExitCode = $LASTEXITCODE
    $ErrorActionPreference = $previousErrorAction
    if ($buildExitCode -ne 0) { throw "Server build failed ($buildExitCode)" }
} finally {
    if ($previousErrorAction) { $ErrorActionPreference = $previousErrorAction }
    Pop-Location
}
