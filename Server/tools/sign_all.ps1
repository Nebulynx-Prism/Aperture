$ErrorActionPreference = 'Stop'

$pfx = (Resolve-Path "tools\nebulynx_codesign.pfx").Path
$flags = [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]"Exportable,UserKeySet,PersistKeySet"
$cert = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2($pfx, "nebulynx", $flags)

$targets = @(
    "target\release\prism-aperture-server.exe",
    "target\release\prism-aperture-service.exe",
    "target\release\PrismAperture-Setup.exe",
    "target\release\OpenReality-Setup.exe"
)

foreach ($t in $targets) {
    if (Test-Path $t) {
        $sig = Set-AuthenticodeSignature -Certificate $cert -FilePath $t -HashAlgorithm SHA256
        $check = Get-AuthenticodeSignature $t
        Write-Host "File: $(Split-Path -Leaf $t) | Signer: $($check.SignerCertificate.Subject) | Status: $($check.Status)"
    }
}
