$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.Security

$rsa = [System.Security.Cryptography.RSA]::Create(2048)
$dn = New-Object System.Security.Cryptography.X509Certificates.X500DistinguishedName('CN=Nebulynx Game Studios, O=Nebulynx Game Studios, C=US')
$req = New-Object System.Security.Cryptography.X509Certificates.CertificateRequest(
    $dn,
    $rsa,
    [System.Security.Cryptography.HashAlgorithmName]::SHA256,
    [System.Security.Cryptography.RSASignaturePadding]::Pkcs1
)

# Key usage: Digital Signature
$req.CertificateExtensions.Add((New-Object System.Security.Cryptography.X509Certificates.X509KeyUsageExtension(
    [System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]::DigitalSignature,
    $true
)))

# Enhanced Key Usage: Code Signing (1.3.6.1.5.5.7.3.3)
$oids = New-Object System.Security.Cryptography.OidCollection
$oids.Add((New-Object System.Security.Cryptography.Oid('1.3.6.1.5.5.7.3.3', 'Code Signing'))) | Out-Null
$req.CertificateExtensions.Add((New-Object System.Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension(
    $oids,
    $true
)))

$notBefore = [System.DateTimeOffset]::UtcNow.AddDays(-1)
$notAfter = [System.DateTimeOffset]::UtcNow.AddYears(10)
$cert = $req.CreateSelfSigned($notBefore, $notAfter)

$pfxBytes = $cert.Export([System.Security.Cryptography.X509Certificates.X509ContentType]::Pfx, 'nebulynx')
[System.IO.File]::WriteAllBytes('tools\nebulynx_codesign.pfx', $pfxBytes)

Write-Host "Created tools\nebulynx_codesign.pfx successfully! ($($pfxBytes.Length) bytes)"
