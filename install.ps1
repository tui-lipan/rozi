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
# The advertised install URL, quoted back to a user who needs to re-run with -AddToPath. The
# release location is overridable for mirrors; the script's own address is not, so this is a
# constant rather than an environment lookup.
$script:InstallScriptUrl = 'https://rozi.tui-lipan.dev/install.ps1'
$script:MaxArchiveBytes = [int64]268435456
$script:MaxChecksumBytes = [int64]1048576
$script:MaxZipMemberBytes = [int64]268435456
$script:MaxZipTotalBytes = [int64]268435456
$script:Caveat = 'Downloading an archive and its checksum from the same HTTPS release location protects against corruption, but does not provide independent authenticity if the release account or release assets are compromised.'

# A console gets one rewritten active row. NO_COLOR disables styling, not the compact interaction.
# Redirected and CI output remains a normal, append-only transcript.
$script:Interactive = -not [Console]::IsOutputRedirected
$script:Esc = if ($script:Interactive) { [char]27 } else { '' }
if ($script:Interactive -and -not $env:NO_COLOR) {
    $script:CReset = "$($script:Esc)[0m"
    # The rozi palette, matching `platform::ansi::palette` and the logo's rose-to-violet gradient.
    $script:CDim = "$($script:Esc)[38;2;142;147;180m"
    $script:CAccent = "$($script:Esc)[38;2;253;74;128m"
    $script:CBand2 = "$($script:Esc)[38;2;228;66;156m"
    $script:CBand3 = "$($script:Esc)[38;2;203;58;185m"
    $script:CViolet = "$($script:Esc)[38;2;178;51;213m"
    # The unfilled remainder of the meter, near the app's border colour so a track reads as chrome.
    $script:CTrack = "$($script:Esc)[38;2;52;56;88m"
    $script:COk = "$($script:Esc)[38;2;74;222;128m"
    $script:CError = "$($script:Esc)[38;2;255;95;87m"
} else {
    $script:CReset = ''
    $script:CDim = ''
    $script:CAccent = ''
    $script:CBand2 = ''
    $script:CBand3 = ''
    $script:CViolet = ''
    $script:CTrack = ''
    $script:COk = ''
    $script:CError = ''
}
$script:CurrentOperation = ''

# Built from code points rather than written as literals, so this file is pure ASCII. PowerShell
# 5.1 reads a script without a byte-order mark as the system ANSI code page, which mangles a UTF-8
# glyph in the source whenever the file is run from disk - the documented `.\install.ps1` form -
# however cleanly the site serves it over the wire. A BOM would fix that case and break a worse
# one: `iex` treats a leading U+FEFF as part of the first token, so `irm ... | iex` would fail
# outright on any body whose BOM survived the fetch. ASCII source has neither problem.
$script:GlyphActive = [string][char]0x25CF  # BLACK CIRCLE
$script:GlyphOk     = [string][char]0x2713  # CHECK MARK
$script:GlyphFailed = [string][char]0x2717  # BALLOT X
$script:GlyphFill   = [string][char]0x2501  # BOX DRAWINGS HEAVY HORIZONTAL
$script:GlyphTrack  = [string][char]0x2500  # BOX DRAWINGS LIGHT HORIZONTAL
$script:GlyphSep    = [string][char]0x00B7  # MIDDLE DOT

# Deliberately ASCII, and character-for-character the same wordmark install.sh prints. A Windows
# console under a non-UTF-8 code page mangles box-drawing and block characters.
function Show-Banner {
    $art = @(
        '                _ ',
        '  _ __ ___ ___ (_)',
        " | '__/ _ \_  /| |",
        ' | | | (_) / / | |',
        ' |_|  \___/___||_|'
    )
    Write-Host ''
    foreach ($line in $art) { Write-Host "$($script:CDim)$line$($script:CReset)" }
    Write-Host ''
}

function Write-StatusRow([string]$Symbol, [string]$Color, [string]$Operation, [string]$Detail) {
    $row = "  $Color$Symbol$($script:CReset) $($Operation.PadRight(12))$Detail"
    if ($script:Interactive) {
        Write-Host -NoNewline "`r$($script:Esc)[2K$row"
    } else {
        Write-Host $row
    }
}

function Write-Active([string]$Operation, [string]$Detail) {
    $script:CurrentOperation = $Operation
    Write-StatusRow $script:GlyphActive $script:CAccent $Operation $Detail
}

function Write-Done([string]$Operation, [string]$Detail) {
    Write-StatusRow $script:GlyphOk $script:COk $Operation $Detail
    if ($script:Interactive) { Write-Host '' }
    $script:CurrentOperation = ''
}

function Write-Failed([string]$Operation, [string]$Detail) {
    Write-StatusRow $script:GlyphFailed $script:CError $Operation $Detail
    if ($script:Interactive) { Write-Host '' }
    $script:CurrentOperation = ''
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


# `irm ... | iex` runs this text in the caller's own session, where a top-level `exit` terminates
# *their* shell rather than this script. That closed the terminal on every install, successful or
# not, and handed the installer's status back as the shell's own exit code - a failed install took
# the window down with `exit 1`. `$PSCommandPath` is set only when there is a real script
# invocation to leave: it is empty under `iex`, including an `iex` nested inside another script,
# where `$MyInvocation.MyCommand.CommandType` still reports `ExternalScript` and would mislead.
function Exit-Installer {
    $global:LASTEXITCODE = $script:ExitCode
    if ($PSCommandPath) { exit $script:ExitCode }
}
# 1 enforces, 2 evaluates, and the key is absent on Windows builds that predate the feature.
function Get-AppControlState {
    try {
        $policy = Get-ItemProperty -LiteralPath 'HKLM:\SYSTEM\CurrentControlSet\Control\CI\Policy' `
            -Name 'VerifiedAndReputablePolicyState' -ErrorAction Stop
    } catch {
        return ''
    }
    switch ($policy.VerifiedAndReputablePolicyState) {
        1 { 'Smart App Control is on' }
        2 { 'Smart App Control is in evaluation mode' }
        default { '' }
    }
}

# Windows can refuse to *start* the payload - Smart App Control and WDAC block unsigned binaries
# that carry no established reputation - and that is a property of the machine, not of the release.
# Saying "signature failed" there blames an archive nothing has examined: the Ed25519 check runs
# inside the payload, which is precisely what did not run. The message is localized by Windows, so
# the caller discriminates on the exception type rather than on any text in it.
function Format-LaunchRefusal($ErrorRecord) {
    # `Exception.Message` has the script position glued onto its tail; the inner Win32Exception
    # carries the operating system's reason on its own, which is the only part worth showing.
    $inner = $ErrorRecord.Exception.InnerException
    $reason = if ($inner) {
        $inner.Message.Trim()
    } else {
        ($ErrorRecord.Exception.Message -split "`r?`n", 2)[0].Trim()
    }
    $lines = @(
        'Windows refused to run the extracted payload, so the release was never verified.',
        "  $reason"
    )
    $policy = Get-AppControlState
    if ($policy) {
        $lines += "  $policy, and it blocks unsigned executables that carry no reputation."
    }
    $lines += '  The archive downloaded and its SHA-256 matched. This is a local execution policy,'
    $lines += '  not a bad release.'
    $lines -join [Environment]::NewLine
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
                        # Floor rather than round: a rounded 99.6 reads as 100 while bytes are
                        # still arriving, which fills the bar early and defeats the reserved cell.
                        $percent = [int][Math]::Floor(100 * $total / $response.ContentLength)
                        if ($percent -ne $lastPercent) {
                            $lastPercent = $percent
                            # Filled run and track differ in weight *and* colour: the weight
                            # survives NO_COLOR, the colour makes the boundary obvious. The filled
                            # run steps through the logo's rose-to-violet gradient in four bands.
                            $width = 32
                            $filled = [int][Math]::Floor($percent * $width / 100)
                            # Reserve the last cell until the download is actually complete, so the
                            # meter never reads full while bytes are still arriving.
                            if ($percent -lt 100 -and $filled -ge $width) { $filled = $width - 1 }
                            $remaining = $width - $filled
                            $bar = ''
                            if ($filled -gt 0) {
                                if ($script:CAccent) {
                                    $bands = @($script:CAccent, $script:CBand2, $script:CBand3, $script:CViolet)
                                    for ($cell = 0; $cell -lt $filled; $cell++) {
                                        # `[int]` rounds in PowerShell rather than truncating, so
                                        # `[int](28 * 4 / 32)` is `[int]3.5` = 4 - one past the end
                                        # of a four-entry array, which failed every interactive
                                        # Windows download at exactly 90%. install.sh gets this
                                        # free from truncating arithmetic and a `*)` catch-all; the
                                        # port has to say it, and the clamp keeps the last band
                                        # covering the tail however the width divides.
                                        $band = [int][Math]::Floor($cell * $bands.Count / $width)
                                        if ($band -ge $bands.Count) { $band = $bands.Count - 1 }
                                        $bar += $bands[$band] + $script:GlyphFill
                                    }
                                } else {
                                    $bar = $script:GlyphFill * $filled
                                }
                            }
                            if ($remaining -gt 0) { $bar += $script:CTrack + ($script:GlyphTrack * $remaining) }
                            Write-StatusRow $script:GlyphActive $script:CAccent 'Download' "$bar$($script:CReset) $percent%"
                        }
                    }
                }
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
    Write-Active 'Signature' 'verifying signed release'
    $output = (& $Payload install 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        $output = $output -replace '^rozi: installation failed:\s*', ''
        if ($output -match '(?i)release verification error|certificate|signature') {
            Write-Failed 'Signature' 'verification failed'
        } else {
            Write-Failed 'Install' 'activation failed'
        }
        Fail $output
    }
    Write-Done 'Signature' 'Ed25519 verified'
    Write-Active 'Install' 'activating command'
    Write-Done 'Install' '%LOCALAPPDATA%\rozi\bin\rozi.exe'
}

# Answer whether a PATH value already names `$Directory`, comparing entries the way Windows
# resolves them: case-insensitively, and without caring about a trailing separator.
function Test-PathContainsDirectory([string]$PathValue, [string]$Directory) {
    $wanted = $Directory.TrimEnd([char]'\')
    foreach ($entry in @($PathValue -split ';')) {
        if ($entry.Trim().TrimEnd([char]'\') -ieq $wanted) {
            return $true
        }
    }
    return $false
}

function Get-ManagedBinDirectory {
    if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        return ''
    }
    Join-Path $env:LOCALAPPDATA 'rozi\bin'
}

# `rozi` as a bare word only resolves if the managed directory is on PATH, and this script does not
# put it there unless asked. Printing `$ rozi` unconditionally handed a user whose PATH does not
# carry it a command that cannot be found, with nothing to say why.
#
# The persisted user PATH and this process's PATH are reported separately because they diverge in a
# way that matters here: an entry added now cannot reach a shell that started before it, so a setup
# that is correct for every future terminal still needs the full path in this one. Saying "not on
# PATH" there would be wrong, and saying nothing would be worse.
function Write-CommandHint([string]$ManagedBin) {
    if (-not $ManagedBin) {
        Write-Host "  $($script:CDim)`$ rozi$($script:CReset)"
        return
    }
    $command = Join-Path $ManagedBin 'rozi.exe'
    $inSession = Test-PathContainsDirectory ([Environment]::GetEnvironmentVariable('Path', 'Process')) $ManagedBin
    $inUser = Test-PathContainsDirectory ([Environment]::GetEnvironmentVariable('Path', 'User')) $ManagedBin

    if ($inSession) {
        Write-Host "  $($script:CDim)`$ rozi$($script:CReset)"
        return
    }
    Write-Host "  $($script:CDim)`$ $command$($script:CReset)"
    Write-Host ''
    if ($inUser) {
        Write-Host '  rozi is on PATH for new terminals. This one started before it was added, so'
        Write-Host '  the full path above is what works here.'
        return
    }
    Write-Host '  rozi is not on your PATH. To put it there, re-run with -AddToPath:'
    Write-Host "  $($script:CDim)& ([scriptblock]::Create((irm $($script:InstallScriptUrl)))) -AddToPath$($script:CReset)"
}

function Add-ManagedBinToPath {
    $managedBin = Get-ManagedBinDirectory
    if (-not $managedBin) {
        Fail 'LOCALAPPDATA is unavailable; cannot add the managed command to PATH'
    }
    if (-not (Test-Path -LiteralPath $managedBin -PathType Container)) {
        Fail "managed command directory is missing after install: $managedBin"
    }

    # The persisted entry is what every future terminal inherits; the process entry is what this
    # one can still use. Both are set, and both are checked first, so re-running is a no-op rather
    # than a second copy of the same directory.
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not (Test-PathContainsDirectory $userPath $managedBin)) {
        $newUserPath = if ([string]::IsNullOrEmpty($userPath)) {
            $managedBin
        } else {
            "$userPath;$managedBin"
        }
        [Environment]::SetEnvironmentVariable('Path', $newUserPath, 'User')
    }

    $processPath = [Environment]::GetEnvironmentVariable('Path', 'Process')
    if (-not (Test-PathContainsDirectory $processPath $managedBin)) {
        $env:Path = if ([string]::IsNullOrEmpty($processPath)) {
            $managedBin
        } else {
            "$processPath;$managedBin"
        }
    }
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
        Write-Active 'Download' $archiveName
        Download-HttpsFile "$base/$archiveName" $archive $script:MaxArchiveBytes $true
        Download-HttpsFile "$base/$archiveName.sha256" $checksum $script:MaxChecksumBytes
        Write-Done 'Download' $archiveName
        Assert-Size $archive $script:MaxArchiveBytes 'release archive'
        Assert-Size $checksum $script:MaxChecksumBytes 'checksum'
        Write-Active 'Checksum' 'verifying SHA-256'
        Verify-Checksum $archive $checksum
        Write-Done 'Checksum' 'SHA-256 verified'
        $payload = Join-Path $temporaryRoot 'payload.exe'
        $launcher = Join-Path $temporaryRoot 'launcher.exe'
        Inspect-And-ExtractZip $archive $stem $payload $launcher
        Assert-RegularFile $payload 'archive payload'
        Assert-RegularFile $launcher 'archive launcher'
        Assert-Size $payload $script:MaxZipMemberBytes 'archive payload'
        Assert-Size $launcher $script:MaxZipMemberBytes 'archive launcher'
        # Not 'Signature': the signed-release check runs inside the payload, further down in
        # `Invoke-ManagedCli`. This row is the payload sanity probe, and labelling it correctly is
        # what keeps a refused launch from being reported as a bad signature.
        Write-Active 'Payload' 'checking archive payload'
        try {
            $reportedVersion = (& $payload --version 2>&1 | Out-String)
        } catch [System.Management.Automation.ApplicationFailedException] {
            # Only a failure to *start* the process lands here. `$ErrorActionPreference = 'Stop'`
            # makes that terminating before `$LASTEXITCODE` is ever assigned, so the check below
            # cannot see it. A payload that runs and merely exits non-zero still takes that path.
            Fail (Format-LaunchRefusal $_)
        }
        $versionLine = ($reportedVersion -split "`r?`n", 2)[0]
        if ($LASTEXITCODE -ne 0) {
            Fail "archive payload could not report its version: $($reportedVersion.Trim())"
        }
        if ($versionLine -cne "rozi $ResolvedVersion") {
            Fail "archive payload version does not match requested release $ResolvedVersion"
        }
        Invoke-ManagedCli $payload
        if ($AddPath) { Add-ManagedBinToPath }

        Write-Host ''
        Write-Host "  rozi $ResolvedVersion installed successfully"
        Write-Host ''
        Write-CommandHint (Get-ManagedBinDirectory)
        Write-Host ''
    } finally {
        if (Test-Path -LiteralPath $temporaryRoot) {
            Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
        }
    }
}

if ($Help) {
    Show-Usage
    $script:ExitCode = 0
    Exit-Installer
    return
}

try {
    Show-Banner
    $resolvedDetail = ''
    if (-not $Version) {
        Write-Active 'Resolve' 'latest release'
        $Version = Resolve-LatestVersion
        $resolvedDetail = "latest release $Version"
    } else {
        $resolvedDetail = "release $Version"
    }
    Assert-Version $Version
    $target = Get-Target
    if ($script:Interactive) { Write-Host -NoNewline "`r$($script:Esc)[2K" }
    Write-Host "  $($script:CDim)rozi $Version  $($script:GlyphSep)  $target$($script:CReset)"
    Write-Host ''
    Write-Done 'Resolve' $resolvedDetail
    Install-Version $Version ([bool]$AddToPath)
    $script:ExitCode = 0
} catch {
    if ($script:CurrentOperation) {
        Write-Failed $script:CurrentOperation 'failed'
    }
    [Console]::Error.WriteLine('')
    [Console]::Error.WriteLine('installation failed')
    [Console]::Error.WriteLine($_.Exception.Message)
}

Exit-Installer
