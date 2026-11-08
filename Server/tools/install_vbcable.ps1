[CmdletBinding()]
param([switch]$VerifyOnly, [string]$CacheDirectory = (Join-Path $env:TEMP ('Aperture-VBCable-' + [guid]::NewGuid().ToString('N'))))
$ErrorActionPreference = 'Stop'
# Executed only if the user selects the optional installer component.
$uri = 'https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack45.zip'
$expectedHash = 'B950E39F01AF1D04EA623C8F6D8EB9B6EA5C477C637295FABF20631C85116BFB'
New-Item -ItemType Directory -Force -Path $CacheDirectory | Out-Null
$archive = Join-Path $CacheDirectory 'VBCABLE_Driver_Pack45.zip'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
if (!(Test-Path -LiteralPath $archive)) {
    Invoke-WebRequest -UseBasicParsing -Uri $uri -OutFile $archive -TimeoutSec 120
}
if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $expectedHash) {
    throw 'VB-CABLE archive hash mismatch; refusing to launch. Download package may have changed.'
}
$expanded = Join-Path $CacheDirectory 'package'
Expand-Archive -LiteralPath $archive -DestinationPath $expanded -Force
$setup = Join-Path $expanded 'VBCABLE_Setup_x64.exe'
$signature = Get-AuthenticodeSignature -LiteralPath $setup
if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'CN=BUREL VINCENT Entrepreneur individuel') {
    throw 'VB-CABLE publisher signature verification failed; refusing to launch.'
}
if ($VerifyOnly) { Write-Output 'Verified archive and publisher; nothing installed.'; exit 0 }
# Use the vendor UI, including its license/reboot notices; never hide prompts.
$process = Start-Process -FilePath $setup -WorkingDirectory $expanded -Wait -PassThru
if ($process.ExitCode -notin @(0, 3010)) { throw "VB-CABLE setup exited with $($process.ExitCode). Check vendor setup results." }
Write-Output 'VB-CABLE setup finished. Restart Windows before selecting the cable in Aperture.'
