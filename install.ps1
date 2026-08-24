[CmdletBinding()]
param(
    [string]$Version,
    [switch]$AddToPath,
    [switch]$Help
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Add-Type -AssemblyName System.IO.Compression

# Trust-boundary caveat: "Downloading an archive and its checksum from the same HTTPS release location protects against corruption, but does not provide independent authenticity if the release account or release assets are compromised."
$script:ExitCode = 1
$script:ReleaseRepo = if ($env:ROZI_RELEASE_REPO) { $env:ROZI_RELEASE_REPO } else { 'tui-lipan/rozi' }
$script:MaxArchiveBytes = [int64]268435456
$script:MaxChecksumBytes = [int64]1048576
$script:MaxZipMemberBytes = [int64]268435456
$script:MaxZipTotalBytes = [int64]268435456
$script:Caveat = 'Downloading an archive and its checksum from the same HTTPS release location protects against corruption, but does not provide independent authenticity if the release account or release assets are compromised.'

# Colour and progress only when the console is really a console. `irm … | iex` still qualifies: the
# pipeline is inside PowerShell, not on stdout. A redirected install writes a transcript someone
# reads later, so it gets plain text and no progress line.
$script:Interactive = (-not [Console]::IsOutputRedirected) -and (-not $env:NO_COLOR)
if ($script:Interactive) {
    $script:Esc = [char]27
    $script:CReset = "$($script:Esc)[0m"
    $script:CDim = "$($script:Esc)[90m"
    $script:CAccent = "$($script:Esc)[1;36m"
    $script:COk = "$($script:Esc)[1;32m"
    $script:CBold = "$($script:Esc)[1m"
} else {
    $script:CReset = ''
    $script:CDim = ''
    $script:CAccent = ''
    $script:COk = ''
    $script:CBold = ''
}

# Deliberately ASCII, and character-for-character the same wordmark install.sh prints. A Windows
# console under a non-UTF-8 code page mangles box-drawing and block characters.
function Show-Banner {
    if (-not $script:Interactive) { return }
    $art = @(
        '                _ ',
        '  _ __ ___ ___ (_)',
        " | '__/ _ \_  /| |",
        ' | | | (_) / / | |',
        ' |_|  \___/___||_|'
    )
    Write-Host ''
    foreach ($line in $art) { Write-Host "$($script:CAccent)$line$($script:CReset)" }
    Write-Host ''
}

function Write-Step([string]$Message) {
    Write-Host "  $($script:CDim)->$($script:CReset)  $Message"
}

function Write-Ok([string]$Message) {
    Write-Host "  $($script:COk)ok$($script:CReset)  $Message"
}

# Wrap to a readable measure rather than emitting one long paragraph the console hard-wraps at an
# arbitrary column. Each line closes its own styling, so an interrupted run leaves the console as it
# was found.
function Write-Wrapped([string]$Prefix, [string]$Text) {
    $width = 74
    $line = ''
    foreach ($word in ($Text -split '\s+' | Where-Object { $_ })) {
        if (-not $line) {
            $line = $word
        } elseif (($line.Length + 1 + $word.Length) -le $width) {
            $line = "$line $word"
        } else {
            Write-Host "  $Prefix$line$($script:CReset)"
            $line = $word
        }
    }
    if ($line) { Write-Host "  $Prefix$line$($script:CReset)" }
}

function Show-Usage {
    @"
Usage:
  .\install.ps1 [-Version VERSION] [-AddToPath]

The default version is the current GitHub release. -Version selects an exact release archive.
ROZI_RELEASE_BASE_URL may point at an HTTPS release mirror, and ROZI_RELEASE_LATEST_URL
selects an HTTPS /releases/latest redirect endpoint whose final URL must contain a v-prefixed tag.

After bootstrap verification, this script executes the extracted payload with `install`. The
installed CLI owns the managed versions, active selector, launcher, rollback metadata, and command
path; this script does not create any of those files. User PATH is changed only after a successful
install when -AddToPath is passed.

Use the installed command for lifecycle operations:
  rozi update --check
  rozi update
  rozi update --rollback

Trust-boundary caveat: "$($script:Caveat)"

Exit status: 0 means success; 1 means download, checksum, archive, or install failure; 2 means
invalid command-line usage.
"@
}

function Fail([string]$Message) {
    throw $Message
}

function Usage-Error([string]$Message) {
    $script:ExitCode = 2
    throw $Message
}

function Assert-Https([string]$Url) {
    try {
        $uri = [Uri]$Url
    } catch {
        Fail "invalid URL: $Url"
    }
    if ($uri.Scheme -ne 'https') {
        Fail "URL must use HTTPS: $Url"
    }
}

function Assert-Version([string]$Value) {
    if ([string]::IsNullOrWhiteSpace($Value) -or $Value -notmatch '^[0-9]+\.[0-9]+\.[0-9]+([-.+][0-9A-Za-z.-]+)?$') {
        Usage-Error "invalid release version: $Value"
    }
}

function Assert-LatestEndpoint([string]$Url) {
    $uri = [Uri]$Url
    if ($uri.AbsolutePath.TrimEnd('/') -notmatch '/releases/latest$') {
        Fail 'latest-release URL must end in /releases/latest'
    }
}

function Resolve-LatestVersion {
    $current = if ($env:ROZI_RELEASE_LATEST_URL) {
        $env:ROZI_RELEASE_LATEST_URL
    } else {
        "https://github.com/$($script:ReleaseRepo)/releases/latest"
    }
    Assert-Https $current
    Assert-LatestEndpoint $current
    for ($attempt = 0; $attempt -le 5; $attempt++) {
        $request = [Net.HttpWebRequest]::Create($current)
        $request.AllowAutoRedirect = $false
        $request.Method = 'GET'
        $request.UserAgent = 'rozi-bootstrap'
        try {
            $response = $request.GetResponse()
        } catch {
            Fail "could not resolve the current released version: $($_.Exception.Message)"
        }
        try {
            $status = [int]$response.StatusCode
            if ($status -ge 300 -and $status -lt 400) {
                $location = $response.Headers['Location']
                if (-not $location) { Fail "latest-release redirect has no Location header: $current" }
                $current = ([Uri]::new([Uri]$current, $location)).AbsoluteUri
                Assert-Https $current
                continue
            }
            if ($status -lt 200 -or $status -ge 300) {
                Fail "unexpected HTTP status $status for latest release: $current"
            }
            $tag = ([Uri]$current).AbsolutePath.TrimEnd('/').Split('/')[-1]
            if ($tag -notmatch '^v') {
                Fail 'latest-release URL did not resolve to a v-prefixed tag'
            }
            $resolved = $tag.Substring(1)
            Assert-Version $resolved
            return $resolved
        } finally {
            $response.Dispose()
        }
    }
    Fail "too many HTTPS redirects for latest release: $current"
}

function Get-Target {
    switch ($env:PROCESSOR_ARCHITECTURE.ToUpperInvariant()) {
        'AMD64' { return 'x86_64-pc-windows-msvc' }
        'ARM64' { Fail 'unsupported Windows architecture: ARM64 (no aarch64-pc-windows-msvc release asset)' }
        default { Fail "unsupported Windows architecture: $($env:PROCESSOR_ARCHITECTURE)" }
    }
}

function Get-ReleaseBase([string]$ResolvedVersion) {
    $base = if ($env:ROZI_RELEASE_BASE_URL) {
        $env:ROZI_RELEASE_BASE_URL
    } else {
        "https://github.com/$($script:ReleaseRepo)/releases/download/v$ResolvedVersion"
    }
    Assert-Https $base
    return $base.TrimEnd('/')
}

function Download-HttpsFile([string]$Url, [string]$Destination, [int64]$MaxBytes, [bool]$ShowProgress = $false) {
    $current = $Url
    for ($attempt = 0; $attempt -le 5; $attempt++) {
        Assert-Https $current
        $request = [Net.HttpWebRequest]::Create($current)
        $request.AllowAutoRedirect = $false
        $request.Method = 'GET'
        $request.UserAgent = 'rozi-bootstrap'
        try {
            $response = $request.GetResponse()
        } catch {
            Fail "download failed for $current`: $($_.Exception.Message)"
        }
        try {
            $status = [int]$response.StatusCode
            if ($status -ge 300 -and $status -lt 400) {
                $location = $response.Headers['Location']
                if (-not $location) { Fail "HTTPS redirect has no Location header: $current" }
                $current = ([Uri]::new([Uri]$current, $location)).AbsoluteUri
                continue
            }
            if ($status -lt 200 -or $status -ge 300) {
                Fail "unexpected HTTP status $status for $current"
            }
            if ($response.ContentLength -ge 0 -and $response.ContentLength -gt $MaxBytes) {
                Fail "download exceeds its size limit: $current"
            }
            $input = $response.GetResponseStream()
            $output = [IO.File]::Open($Destination, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
            $buffer = New-Object byte[] 65536
            [int64]$total = 0
            # Only the release archive is worth a progress line: it is several megabytes and was
            # previously silent long enough to look hung. A declared length is required to show a
            # percentage, and the line is rewritten in place so it collapses to one row.
            $showProgress = $ShowProgress -and $script:Interactive -and ($response.ContentLength -gt 0)
            $lastPercent = -1
            try {
                while (($read = $input.Read($buffer, 0, $buffer.Length)) -gt 0) {
                    if ($total -gt ($MaxBytes - $read)) {
                        Fail "download exceeds its size limit: $current"
                    }
                    $output.Write($buffer, 0, $read)
                    $total += $read
                    if ($showProgress) {
                        $percent = [int](100 * $total / $response.ContentLength)
                        if ($percent -ne $lastPercent) {
                            $lastPercent = $percent
                            $filled = [int]($percent / 2)
                            $bar = ('#' * $filled).PadRight(50)
                            Write-Host -NoNewline "`r  $($script:CDim)$bar$($script:CReset) $percent%"
                        }
                    }
                }
                if ($showProgress) { Write-Host "`r$(' ' * 62)`r" -NoNewline }
                if ($response.ContentLength -ge 0 -and $total -ne $response.ContentLength) {
                    Fail "download ended before its declared length: $current"
                }
            } finally {
                $output.Dispose()
                $input.Dispose()
            }
            return
        } finally {
            $response.Dispose()
        }
    }
    Fail "too many HTTPS redirects for $Url"
}

function Verify-Checksum([string]$Archive, [string]$Checksum) {
    $checksumLength = (Get-Item -LiteralPath $Checksum).Length
    if ($checksumLength -gt $script:MaxChecksumBytes) {
        Fail "checksum exceeds its size limit: $Checksum"
    }
    $nonEmpty = @(Get-Content -LiteralPath $Checksum | Where-Object { $_.Trim().Length -gt 0 })
    if ($nonEmpty.Count -ne 1 -or $nonEmpty[0] -notmatch '^\s*(?<hash>[0-9A-Fa-f]{64})\s+(?<name>\*?[^\s]+)\s*$') {
        Fail "malformed checksum file: $Checksum"
    }
    $expected = $Matches.hash.ToLowerInvariant()
    $listed = $Matches.name.TrimStart('*')
    if ($listed -ne [IO.Path]::GetFileName($Archive)) {
        Fail "checksum names a different archive: $listed"
    }
    $actual = (Get-FileHash -LiteralPath $Archive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        Fail "archive checksum does not match $Checksum"
    }
}

function Assert-ZipEntryName([string]$Name, [string]$Stem) {
    if ([string]::IsNullOrEmpty($Name) -or $Name.Contains('\') -or $Name.Contains('//') -or $Name.Contains([char]0)) {
        Fail "unsafe path in release archive: $Name"
    }
    if ($Name.StartsWith('/')) {
        Fail "unsafe path in release archive: $Name"
    }
    $member = $Name.TrimEnd('/')
    if ([string]::IsNullOrEmpty($member) -or $member -match '(^|/)\.\.?(/|$)') {
        Fail "unsafe path in release archive: $Name"
    }
    if ($member -ne $Stem -and -not $member.StartsWith("$Stem/")) {
        Fail "archive member escapes canonical root: $Name"
    }
}

function Assert-RegularFile([string]$Path, [string]$Label) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        Fail "$Label is missing: $Path"
    }
    $item = Get-Item -LiteralPath $Path
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        Fail "$Label must not be a reparse point: $Path"
    }
}

function Assert-Size([string]$Path, [int64]$MaxBytes, [string]$Label) {
    $length = (Get-Item -LiteralPath $Path).Length
    if ($length -gt $MaxBytes) {
        Fail "$Label exceeds its size limit: $Path"
    }
}

function Assert-ZipEntryAttributes([System.IO.Compression.ZipArchiveEntry]$Entry) {
    [uint32]$attributes = [uint32]$Entry.ExternalAttributes
    [uint32]$unixMode = ($attributes -shr 16) -band 0xffff
    [uint32]$unixType = $unixMode -band 0xf000
    [uint32]$dosAttributes = $attributes -band 0xffff
    if ($unixType -ne 0 -and $unixType -notin @(0x4000, 0x8000)) {
        Fail "archive contains a symlink or special ZIP entry: $($Entry.FullName)"
    }
    if ($unixType -eq 0x4000 -and -not $Entry.FullName.EndsWith('/')) {
        Fail "directory ZIP entry has no directory name: $($Entry.FullName)"
    }
    if ($unixType -eq 0x8000 -and $Entry.FullName.EndsWith('/')) {
        Fail "regular ZIP entry has a directory name: $($Entry.FullName)"
    }
    if (($dosAttributes -band 0x10) -ne 0 -and -not $Entry.FullName.EndsWith('/')) {
        Fail "DOS directory ZIP entry has no directory name: $($Entry.FullName)"
    }
}

function Copy-ZipEntry(
    [System.IO.Compression.ZipArchiveEntry]$Entry,
    [string]$Destination,
    [int64]$MaxBytes
) {
    $input = $null
    $output = $null
    [int64]$total = 0
    try {
        $input = $Entry.Open()
        $output = [IO.File]::Open(
            $Destination,
            [IO.FileMode]::CreateNew,
            [IO.FileAccess]::Write,
            [IO.FileShare]::None
        )
        $buffer = New-Object byte[] 65536
        while (($read = $input.Read($buffer, 0, $buffer.Length)) -gt 0) {
            if ($total -gt ($MaxBytes - $read)) {
                Fail "ZIP member exceeds its size limit: $($Entry.FullName)"
            }
            $output.Write($buffer, 0, $read)
            $total += $read
        }
        if ($total -ne [int64]$Entry.Length) {
            Fail "ZIP member ended before its declared length: $($Entry.FullName)"
        }
    } catch {
        Fail "unsupported or encrypted ZIP entry '$($Entry.FullName)': $($_.Exception.Message)"
    } finally {
        if ($null -ne $input) { $input.Dispose() }
        if ($null -ne $output) { $output.Dispose() }
    }
}

function Inspect-And-ExtractZip(
    [string]$Archive,
    [string]$Stem,
    [string]$PayloadDestination,
    [string]$LauncherDestination
) {
    $archiveStream = $null
    $zip = $null
    try {
        $archiveStream = [IO.File]::Open(
            $Archive,
            [IO.FileMode]::Open,
            [IO.FileAccess]::Read,
            [IO.FileShare]::Read
        )
        try {
            $zip = [System.IO.Compression.ZipArchive]::new(
                $archiveStream,
                [System.IO.Compression.ZipArchiveMode]::Read,
                $true
            )
        } catch {
            Fail "could not open ZIP archive: $($_.Exception.Message)"
        }

        $names = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        $normalizedNames = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        $payloadName = "$Stem/rozi.exe"
        $launcherName = "$Stem/rozi-launcher.exe"
        $payloadEntry = $null
        $launcherEntry = $null
        [int64]$totalUncompressed = 0
        foreach ($entry in $zip.Entries) {
            $name = [string]$entry.FullName
            Assert-ZipEntryName $name $Stem
            if (-not $names.Add($name)) {
                Fail "archive contains duplicate member: $name"
            }
            Assert-ZipEntryAttributes $entry
            try {
                [int64]$memberLength = $entry.Length
            } catch {
                Fail "unsupported or encrypted ZIP entry '$name': $($_.Exception.Message)"
            }
            if ($memberLength -lt 0 -or $memberLength -gt $script:MaxZipMemberBytes) {
                Fail "ZIP member exceeds its size limit: $name"
            }
            if ($totalUncompressed -gt ($script:MaxZipTotalBytes - $memberLength)) {
                Fail 'ZIP archive exceeds its total uncompressed size limit'
            }
            $totalUncompressed += $memberLength
            $isDirectory = $name.EndsWith('/')
            $member = $name.TrimEnd('/')
            if (-not $normalizedNames.Add($member)) {
                Fail "archive contains duplicate normalized member: $name"
            }
            if ($member -eq $Stem -and -not $isDirectory) {
                Fail "archive root is not a directory: $name"
            }
            if ($member -eq $payloadName) {
                if ($isDirectory) { Fail "archive payload is a directory: $name" }
                $payloadEntry = $entry
            } elseif ($member -eq $launcherName) {
                if ($isDirectory) { Fail "archive launcher is a directory: $name" }
                $launcherEntry = $entry
            }
        }
        if ($null -eq $payloadEntry) {
            Fail "archive has no canonical payload: $payloadName"
        }
        if ($null -eq $launcherEntry) {
            Fail "archive has no canonical launcher: $launcherName"
        }
        Copy-ZipEntry $payloadEntry $PayloadDestination $script:MaxZipMemberBytes
        Copy-ZipEntry $launcherEntry $LauncherDestination $script:MaxZipMemberBytes
    } finally {
        if ($null -ne $zip) { $zip.Dispose() }
        if ($null -ne $archiveStream) { $archiveStream.Dispose() }
    }
}

function Invoke-ManagedCli([string]$Payload) {
    $help = (& $Payload --help 2>&1 | Out-String)
    if ($LASTEXITCODE -ne 0 -or $help -notmatch '(?m)(^|\s)install(?:\s|$)') {
        Fail "verified archive payload has no 'install' command; no managed files were changed"
    }
    & $Payload install
    if ($LASTEXITCODE -ne 0) {
        Fail 'managed installation failed; no bootstrap layout was created by this script'
    }
}

function Add-ManagedBinToPath {
    if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        Fail 'LOCALAPPDATA is unavailable; cannot add the managed command to PATH'
    }
    $managedBin = Join-Path $env:LOCALAPPDATA 'rozi\bin'
    if (-not (Test-Path -LiteralPath $managedBin -PathType Container)) {
        Fail "managed command directory is missing after install: $managedBin"
    }

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $alreadyPresent = $false
    foreach ($entry in @($userPath -split ';')) {
        if ($entry.Trim().TrimEnd([char]'\') -ieq $managedBin.TrimEnd([char]'\')) {
            $alreadyPresent = $true
            break
        }
    }
    if (-not $alreadyPresent) {
        $newUserPath = if ([string]::IsNullOrEmpty($userPath)) {
            $managedBin
        } else {
            "$userPath;$managedBin"
        }
        [Environment]::SetEnvironmentVariable('Path', $newUserPath, 'User')
    }

    $processPath = [Environment]::GetEnvironmentVariable('Path', 'Process')
    $processEntries = @($processPath -split ';')
    $processHasBin = $false
    foreach ($entry in $processEntries) {
        if ($entry.Trim().TrimEnd([char]'\') -ieq $managedBin.TrimEnd([char]'\')) {
            $processHasBin = $true
            break
        }
    }
    if (-not $processHasBin) {
        $env:Path = if ([string]::IsNullOrEmpty($processPath)) {
            $managedBin
        } else {
            "$processPath;$managedBin"
        }
    }
    Write-Host "Added managed command directory to the user PATH: $managedBin"
}

function Install-Version([string]$ResolvedVersion, [bool]$AddPath) {
    $target = Get-Target
    $stem = "rozi-$ResolvedVersion-$target"
    $archiveName = "$stem.zip"
    $base = Get-ReleaseBase $ResolvedVersion
    $temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ('rozi-install-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $temporaryRoot -Force | Out-Null
    try {
        $archive = Join-Path $temporaryRoot $archiveName
        $checksum = "$archive.sha256"
        Assert-Https "$base/$archiveName"
        Assert-Https "$base/$archiveName.sha256"
        Write-Step "downloading $archiveName"
        Download-HttpsFile "$base/$archiveName" $archive $script:MaxArchiveBytes $true
        Download-HttpsFile "$base/$archiveName.sha256" $checksum $script:MaxChecksumBytes
        Assert-Size $archive $script:MaxArchiveBytes 'release archive'
        Assert-Size $checksum $script:MaxChecksumBytes 'checksum'
        Write-Step 'verifying checksum'
        Verify-Checksum $archive $checksum
        Write-Ok 'archive matches its published checksum'
        $payload = Join-Path $temporaryRoot 'payload.exe'
        $launcher = Join-Path $temporaryRoot 'launcher.exe'
        Inspect-And-ExtractZip $archive $stem $payload $launcher
        Assert-RegularFile $payload 'archive payload'
        Assert-RegularFile $launcher 'archive launcher'
        Assert-Size $payload $script:MaxZipMemberBytes 'archive payload'
        Assert-Size $launcher $script:MaxZipMemberBytes 'archive launcher'
        $reportedVersion = (& $payload --version 2>&1 | Out-String)
        $versionLine = ($reportedVersion -split "`r?`n", 2)[0]
        if ($LASTEXITCODE -ne 0 -or $versionLine -cne "rozi $ResolvedVersion") {
            Fail "archive payload version does not match requested release $ResolvedVersion"
        }
        Write-Ok "payload reports $versionLine"

        # The payload prints its own "Installed"/"Command" lines: it is the authority on where the
        # command landed, and repeating it here would only risk disagreeing with it.
        Write-Step 'verifying the signed release and activating it'
        Write-Host ''
        Invoke-ManagedCli $payload
        if ($AddPath) { Add-ManagedBinToPath }

        Write-Host ''
        Write-Host "  $($script:CBold)Start$($script:CReset)    rozi"
        Write-Host "  $($script:CBold)Help$($script:CReset)     rozi --help"
        if (-not $AddPath) {
            $managedBin = Join-Path $env:LOCALAPPDATA 'rozi\bin'
            $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
            $onPath = $userPath -and (($userPath -split ';') | Where-Object {
                $_.Trim().TrimEnd([char]'\') -ieq $managedBin.TrimEnd([char]'\')
            })
            if (-not $onPath) {
                Write-Host ''
                Write-Host "  $($script:CDim)$managedBin is not on your PATH; rerun with -AddToPath to add it.$($script:CReset)"
            }
        }
        Write-Host ''
        Write-Wrapped $script:CDim $script:Caveat
        Write-Host ''
    } finally {
        if (Test-Path -LiteralPath $temporaryRoot) {
            Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
        }
    }
}

if ($Help) {
    Show-Usage
    exit 0
}

try {
    Show-Banner
    if (-not $Version) {
        Write-Step 'resolving the current release'
        $Version = Resolve-LatestVersion
        Write-Ok "latest is $Version"
    }
    Assert-Version $Version
    Install-Version $Version ([bool]$AddToPath)
    $script:ExitCode = 0
} catch {
    Write-Error ("rozi install: " + $_.Exception.Message)
}

exit $script:ExitCode
