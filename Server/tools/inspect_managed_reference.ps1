[CmdletBinding()]
param(
    [string[]]$Paths = @('C:\Program Files\Virtual Desktop\VirtualDesktop.Service.exe', 'C:\Program Files\Virtual Desktop\VirtualDesktop.Server.exe', 'C:\Program Files\Virtual Desktop Streamer\VirtualDesktop.Streamer.exe'),
    [string]$OutputPath = (Join-Path $PSScriptRoot '../docs/managed-reference.json')
)
# Requires Windows PowerShell/.NET Framework. Target assemblies are ONLY reflection-only loaded.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSEdition -ne 'Desktop') { throw 'Run with Windows PowerShell (.NET Framework)' }
$limits = @{ typesPerAssembly = 8; methodsPerType = 5; methodsPerAssembly = 25; ilBytesPerMethod = 2048; instructionsPerMethod = 64; frameworkDependencies = 128; dependencyErrors = 40 }
$dependencyErrors = [Collections.Generic.List[string]]::new()
$opcodes = @{}
# Read opcode descriptors from the trusted framework, never from target fields.
foreach ($field in [Reflection.Emit.OpCodes].GetFields([Reflection.BindingFlags]'Public,Static')) {
    $op = $field.GetValue($null)
    $opcodes[[int]$op.Value -band 65535] = $op
}
function Clip([object]$Value, [int]$Max = 300) {
    $text = [string]$Value
    if ($text.Length -gt $Max) { return $text.Substring(0,$Max) + ' [truncated]' }
    return $text
}
function Get-IlEvidence($Method) {
    $r = [ordered]@{ name = $Method.Name; token = ('0x{0:X8}' -f $Method.MetadataToken); attributes = [string]$Method.Attributes }
    try { $r.signature = Clip ($Method.ToString()) } catch { $r.signatureError = Clip $_ }
    try {
        $body = $Method.GetMethodBody()
        if ($null -eq $body) { $r.bodyPresent = $false; return $r }
        $r.bodyPresent = $true
        [byte[]]$all = $body.GetILAsByteArray()
        $r.totalIlBytes = $all.Length
        $r.maxStack = $body.MaxStackSize
        $r.initLocals = $body.InitLocals
        $r.localCount = $body.LocalVariables.Count
        $r.exceptionClauseCount = $body.ExceptionHandlingClauses.Count
        $count = [Math]::Min($all.Length, $limits.ilBytesPerMethod)
        [byte[]]$bytes = New-Object byte[] $count
        [Array]::Copy($all, $bytes, $count)
        $r.rawIlBase64 = [Convert]::ToBase64String($bytes)
        $r.rawIlTruncated = ($count -lt $all.Length)
        $decoded = [Collections.Generic.List[object]]::new()
        $pos = 0
        while ($pos -lt $bytes.Length -and $decoded.Count -lt $limits.instructionsPerMethod) {
            $offset = $pos
            [int]$key = $bytes[$pos]; $pos++
            if ($key -eq 254) {
                if ($pos -ge $bytes.Length) { throw 'Truncated two-byte opcode' }
                $key = 0xfe00 -bor $bytes[$pos]; $pos++
            }
            if (!$opcodes.ContainsKey($key)) { throw ('Unknown opcode at {0}' -f $offset) }
            $op = $opcodes[$key]
            $operandType = [string]$op.OperandType
            $size = switch ($operandType) {
                'InlineNone' { 0 }
                { $_ -in @('ShortInlineI','ShortInlineVar','ShortInlineBrTarget') } { 1 }
                'InlineVar' { 2 }
                { $_ -in @('InlineI8','InlineR') } { 8 }
                'InlineSwitch' {
                    if ($pos + 4 -gt $bytes.Length) { throw 'Truncated switch count' }
                    $n = [BitConverter]::ToInt32($bytes,$pos)
                    if ($n -lt 0 -or $n -gt 65536) { throw 'Invalid switch count' }
                    4 + 4 * $n
                }
                default { 4 }
            }
            if ($pos + $size -gt $bytes.Length) { throw 'Operand crosses captured IL boundary' }
            $instruction = [ordered]@{ offset = $offset; opcode = $op.Name }
            if ($size -gt 0) {
                $instruction.operandHex = [BitConverter]::ToString($bytes,$pos,[Math]::Min($size,128))
                if ($size -gt 128) { $instruction.operandHexTruncated = $true }
                if ($operandType -in @('InlineMethod','InlineField','InlineType','InlineTok','InlineString','InlineSig')) {
                    $token = [BitConverter]::ToInt32($bytes,$pos)
                    $instruction.token = ('0x{0:X8}' -f $token)
                    try {
                        if ($operandType -eq 'InlineString') {
                            $instruction.resolvedString = Clip ($Method.Module.ResolveString($token))
                        } elseif ($operandType -eq 'InlineSig') {
                            $instruction.resolvedSignatureHex = Clip ([BitConverter]::ToString($Method.Module.ResolveSignature($token)))
                        } else {
                            $member = $Method.Module.ResolveMember($token)
                            $instruction.resolvedMember = Clip ([string]$member)
                            if ($member.DeclaringType) { $instruction.declaringType = $member.DeclaringType.FullName }
                        }
                    } catch { $instruction.resolveError = Clip $_ }
                }
            }
            $decoded.Add($instruction)
            $pos += $size
        }
        $r.decodedInstructions = @($decoded.ToArray())
        $r.decodedThroughByte = $pos
        $r.decodeTruncated = ($pos -lt $all.Length)
    } catch {
        $r.bodyOrDecodeError = Clip $_
        if (Get-Variable decoded -ErrorAction SilentlyContinue) { $r.decodedInstructions = @($decoded.ToArray()) }
    }
    return $r
}
$assemblies = @($Paths | ForEach-Object { [Reflection.Assembly]::ReflectionOnlyLoadFrom($_) })
# Preload dependencies only into reflection-only context. GAC first, then supplied installation roots.
$attempted = @{}
for ($pass = 0; $pass -lt 4; $pass++) {
    foreach ($assembly in [AppDomain]::CurrentDomain.ReflectionOnlyGetAssemblies()) {
        foreach ($reference in $assembly.GetReferencedAssemblies()) {
            if ($attempted.ContainsKey($reference.FullName) -or $attempted.Count -ge $limits.frameworkDependencies) { continue }
            $attempted[$reference.FullName] = $true
            try { $null = [Reflection.Assembly]::ReflectionOnlyLoad($reference.FullName) }
            catch {
                $resolved = $false
                foreach ($path in $Paths) {
                    $candidate = Join-Path (Split-Path $path) ($reference.Name + '.dll')
                    if (Test-Path -LiteralPath $candidate) {
                        try { $null = [Reflection.Assembly]::ReflectionOnlyLoadFrom($candidate); $resolved = $true; break } catch {}
                    }
                }
                if (!$resolved -and $dependencyErrors.Count -lt $limits.dependencyErrors) { $dependencyErrors.Add((Clip "$($reference.FullName): $_" 700)) }
            }
        }
    }
}
$results = foreach ($assembly in $assemblies) {
    $item = Get-Item -LiteralPath $assembly.Location
    $typeErrors = @()
    try { $types = @($assembly.GetTypes()) }
    catch [Reflection.ReflectionTypeLoadException] {
        $types = @($_.Exception.Types | Where-Object { $null -ne $_ })
        $typeErrors = @($_.Exception.LoaderExceptions | Select-Object -First 20 | ForEach-Object { Clip $_ 500 })
    }
    $matches = @($types | Where-Object { $_.FullName -match '^VirtualDesktop\..*(ConnectionManager|NetMessage|DesktopStreamer|ServiceManager|IServiceManager|Service\.Service($|\+)|Server\.(Program|DesktopManager)($|\+))' } | Sort-Object @{Expression={ if ($_.FullName -match '\+') {1} else {0} }},FullName)
    $methodTotal = 0
    $typeResults = foreach ($type in @($matches | Select-Object -First $limits.typesPerAssembly)) {
        $tr = [ordered]@{ name = $type.FullName; token = ('0x{0:X8}' -f $type.MetadataToken); attributes = [string]$type.Attributes }
        try {
            if ($type.BaseType) { $tr.baseType = $type.BaseType.FullName }
            $flags = [Reflection.BindingFlags]'Public,NonPublic,Instance,Static,DeclaredOnly'
            $methods = @(@($type.GetMethods($flags)) + @($type.GetConstructors($flags)) | Sort-Object @{Expression={if ($_.Name -match 'Request|Message|Connect|Stream|Read|Write|Receive|Send|Handle|Start|Stop') {0} else {1}}},MetadataToken)
            $tr.declaredMethodCount = $methods.Count
            $tr.methodInventory = @($methods | Select-Object -First 80 | ForEach-Object { @{ name = $_.Name; token = ('0x{0:X8}' -f $_.MetadataToken) } })
            $tr.methodInventoryTruncated = ($methods.Count -gt 80)
                        # Favor actual implementation bodies over interfaces/event accessors; retain
            # constructors and named service handlers, then largest remaining bodies.
            $bodyCandidates = @($methods | Where-Object { !$_.IsAbstract } | Sort-Object @{Expression={
                if ($_.Name -match '^(DisableProcessThrottling|UseRealtimePriority|Connect|Disconnect|SetOpenXRRuntime|RestartOVRService)$') { 0 }
                elseif ($_.Name -eq '.ctor') { 1 }
                elseif ($_.IsSpecialName) { 3 }
                else { 2 }
            }}, @{Expression={ try { -$_.GetMethodBody().GetILAsByteArray().Length } catch { 0 } }},MetadataToken)
            $remaining = [Math]::Max(0, $limits.methodsPerAssembly - $methodTotal)
            $selected = @($bodyCandidates | Select-Object -First ([Math]::Min($remaining,$limits.methodsPerType)))
            $methodTotal += $selected.Count
            $tr.selectedMethods = @($selected | ForEach-Object { Get-IlEvidence $_ })
            $tr.methodSelectionTruncated = ($selected.Count -lt $methods.Count)
        } catch { $tr.typeInspectionError = Clip $_ 600 }
        $tr
    }
    [ordered]@{
        path = $item.FullName; size = $item.Length; sha256 = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash
        assemblyName = $assembly.FullName; reflectionOnly = $assembly.ReflectionOnly; mvid = $assembly.ManifestModule.ModuleVersionId.ToString()
        availableTypeCount = $types.Count; typeLoadErrors = $typeErrors; matchingTypeCount = $matches.Count
        matchingTypeNames = @($matches | Select-Object -First 100 | ForEach-Object {$_.FullName})
        selectedTypes = @($typeResults); selectedMethodCount = $methodTotal
    }
}
$evidence = [ordered]@{
    schemaVersion = 1; collectedUtc = [DateTime]::UtcNow.ToString('o'); runtime = $PSVersionTable.PSVersion.ToString()
    method = 'ReflectionOnlyLoadFrom/ReflectionOnlyLoad; metadata and bounded raw IL plus opcode decoding. No target method invocation, target field evaluation, target instantiation, custom attribute instantiation, or execution-context target assembly loading.'
    limits = $limits; dependencyAttempts = $attempted.Count; dependencyErrors = @($dependencyErrors.ToArray()); assemblies = @($results)
}
$json = $evidence | ConvertTo-Json -Depth 18
if ($json.Length -gt 3000000) { throw 'Evidence exceeds 3 million character bound' }
[IO.File]::WriteAllText([IO.Path]::GetFullPath($OutputPath),$json,[Text.UTF8Encoding]::new($false))
"Saved $OutputPath ($($json.Length) characters)"



