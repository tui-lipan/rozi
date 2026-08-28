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

# The spinner turns on a background thread, and a thread has no PowerShell host to write through -
# it writes with `[Console]::Write`, which encodes through `[Console]::OutputEncoding`. On a Polish
# console that is code page 852, which has no U+25D0, so every frame arrived as `?` while the ticks
# and bullets the main thread writes came out intact: `Write-Host` reaches the console as wide
# characters and never passes through that encoding at all.
#
# Assigning `OutputEncoding` sets the console's output code page, so this fixes the thread's writes
# and the error path's `[Console]::Error` alike. Only done when the current encoding actually
# cannot carry the glyph, and always put back - the code page outlives this script otherwise.
$script:PreviousOutputEncoding = $null
if ($script:Interactive) {
    try {
        $probe = [string][char]0x25D0
        if ([Console]::OutputEncoding.GetString([Console]::OutputEncoding.GetBytes($probe)) -ne $probe) {
            $script:PreviousOutputEncoding = [Console]::OutputEncoding
            [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
        }
    } catch {
        # A host that refuses the assignment keeps its own encoding; the spinner degrades to `?`
        # rather than the install failing over a decoration.
        $script:PreviousOutputEncoding = $null
    }
}
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
    $script:CWarn = "$($script:Esc)[38;2;251;191;36m"
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
    $script:CWarn = ''
}
$script:CurrentOperation = ''

# Built from code points rather than written as literals, so this file is pure ASCII. PowerShell
# 5.1 reads a script without a byte-order mark as the system ANSI code page, which mangles a UTF-8
# glyph in the source whenever the file is run from disk - the documented `.\install.ps1` form -
# however cleanly the site serves it over the wire. A BOM would fix that case and break a worse
# one: `iex` treats a leading U+FEFF as part of the first token, so `irm ... | iex` would fail
# outright on any body whose BOM survived the fetch. ASCII source has neither problem.
$script:GlyphOk     = [string][char]0x2713  # CHECK MARK
$script:GlyphFailed = [string][char]0x2717  # BALLOT X
$script:GlyphFill   = [string][char]0x2501  # BOX DRAWINGS HEAVY HORIZONTAL
$script:GlyphTrack  = [string][char]0x2500  # BOX DRAWINGS LIGHT HORIZONTAL
$script:GlyphSep    = [string][char]0x00B7  # MIDDLE DOT

# The spinner turns on its own thread, because the work it reports is blocking: `Get-FileHash`, an
# HTTPS request and the payload's own install all hold the pipeline, so a row redrawn by the main
# thread can only advance where the work happens to loop. That made every step but the download
# look frozen, which is worse than no spinner at all.
$script:SpinnerFrames = @(0x25D0, 0x25D3, 0x25D1, 0x25D2) | ForEach-Object { [string][char]$_ }
$script:SpinnerIntervalMs = 50
$script:SpinnerIndex = 0
$script:Spinner = $null

# Paint `$Text` with the rose-to-violet gradient, sampled in four bands across `$Width` - one escape
# per band rather than per character. `$Width` is separate from the text's own length so several
# lines can share one ramp: the wordmark's bands have to line up vertically, which they cannot if
# each line scales the ramp to itself.
function Format-Gradient([string]$Text, [int]$Width = 0) {
    if (-not $script:CAccent) {
        return $Text
    }
    if ($Width -le 0) {
        $Width = $Text.Length
    }
    $bands = @($script:CAccent, $script:CBand2, $script:CBand3, $script:CViolet)
    $painted = ''
    $previous = -1
    for ($column = 0; $column -lt $Text.Length; $column++) {
        # Floors and clamps for the reason the meter's band index does: `[int]` rounds in
        # PowerShell and walks straight off the end of a four-entry array.
        $band = [int][Math]::Floor($column * $bands.Count / $Width)
        if ($band -ge $bands.Count) {
            $band = $bands.Count - 1
        }
        if ($band -ne $previous) {
            $painted += $bands[$band]
            $previous = $band
        }
        $painted += $Text[$column]
    }
    "$painted$($script:CReset)"
}

# Human sizes, so a finished download reports what arrived rather than restating the file name the
# version and target already imply.
function Format-Bytes([int64]$Bytes) {
    # Invariant rather than the current culture: a size is a technical fact that ends up pasted
    # into issue reports, and a Polish console rendering it `7,6 MB` reads as a different number.
    $culture = [Globalization.CultureInfo]::InvariantCulture
    if ($Bytes -ge 1048576) {
        return [string]::Format($culture, '{0:N1} MB', $Bytes / 1048576)
    }
    if ($Bytes -ge 1024) {
        return [string]::Format($culture, '{0:N0} KB', $Bytes / 1024)
    }
    "$Bytes B"
}

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
    # One ramp across the widest line, so the bands line up down the wordmark instead of each row
    # scaling the gradient to its own length.
    $width = ($art | Measure-Object -Property Length -Maximum).Maximum
    foreach ($line in $art) {
        Write-Host (Format-Gradient $line $width)
    }
    Write-Host ''
}

function Write-StatusRow([string]$Symbol, [string]$Color, [string]$Operation, [string]$Detail) {
    $row = " $Color$Symbol$($script:CReset) $($Operation.PadRight(12))$Detail"
    if ($script:Interactive) {
        Write-Host -NoNewline "`r$($script:Esc)[2K$row"
    } else {
        Write-Host $row
    }
}

# Stop the spinner thread and leave the row for whatever writes next.
#
# Idempotent, and called before every write to the row - a second writer mid-frame would interleave
# escape sequences with the finished line.
function Stop-Spinner {
    if (-not $script:Spinner) {
        return
    }
    $spinner = $script:Spinner
    $script:Spinner = $null
    $spinner.Shared.Stop = $true
    try {
        [void]$spinner.Shell.EndInvoke($spinner.Handle)
    } catch {
        # The thread only writes to the console; nothing it can fail at is worth failing an install
        # over, and the row is erased by the next write regardless.
    }
    $spinner.Shell.Dispose()
}

# Draw the active row and, in a console, hand it to a thread that turns it.
#
# The thread is given fully formed strings rather than the script's functions, because it runs in
# its own runspace and would not see them. It writes with `[Console]::Write` for the same reason.
function Write-Active([string]$Operation, [string]$Detail) {
    Stop-Spinner
    $script:CurrentOperation = $Operation
    $script:SpinnerIndex = 0
    Write-StatusRow $script:SpinnerFrames[0] $script:CAccent $Operation $Detail
    if (-not $script:Interactive) {
        return
    }
    $shared = [hashtable]::Synchronized(@{ Stop = $false })
    $shell = [powershell]::Create()
    [void]$shell.AddScript({
        param($shared, $prefix, $suffix, $frames, $intervalMs)
        $index = 0
        while (-not $shared.Stop) {
            Start-Sleep -Milliseconds $intervalMs
            if ($shared.Stop) { break }
            [Console]::Write("$prefix$($frames[$index % $frames.Count])$suffix")
            $index++
        }
    })
    [void]$shell.AddArgument($shared)
    [void]$shell.AddArgument("`r$($script:Esc)[2K $($script:CAccent)")
    [void]$shell.AddArgument("$($script:CReset) $($Operation.PadRight(12))$Detail")
    [void]$shell.AddArgument($script:SpinnerFrames)
    [void]$shell.AddArgument($script:SpinnerIntervalMs)
    $script:Spinner = @{
        Shell  = $shell
        Shared = $shared
        Handle = $shell.BeginInvoke()
    }
}

# Redraw the active row in place, one frame on, for a caller that already owns the row - the
# download meter, which paints a bar the spinner thread knows nothing about.
function Write-Spin([string]$Operation, [string]$Detail) {
    if (-not $script:Interactive) {
        return
    }
    $script:SpinnerIndex = ($script:SpinnerIndex + 1) % $script:SpinnerFrames.Count
    Write-StatusRow $script:SpinnerFrames[$script:SpinnerIndex] $script:CAccent $Operation $Detail
}

function Write-Done([string]$Operation, [string]$Detail) {
    Stop-Spinner
    Write-StatusRow $script:GlyphOk $script:COk $Operation $Detail
    if ($script:Interactive) { Write-Host '' }
    $script:CurrentOperation = ''
}

function Write-Failed([string]$Operation, [string]$Detail) {
    Stop-Spinner
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
    Stop-Spinner
    if ($script:PreviousOutputEncoding) {
        try { [Console]::OutputEncoding = $script:PreviousOutputEncoding } catch { }
        $script:PreviousOutputEncoding = $null
    }
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
            # The meter owns the row from here: it paints a bar the spinner thread knows nothing
            # about, so two writers would interleave escape sequences across it. Its own redraws
            # advance the frame instead, once per percent.
            if ($showProgress) { Stop-Spinner }
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
                            Write-Spin 'Download' "$bar$($script:CReset) $percent%"
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
    # The longest wait in the run, and the row turns through all of it now that the spinner has its
    # own thread - which is why this went back to a plain call. Polling the process here bought the
    # same animation at the cost of redirected streams and a wait loop to get wrong.
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
# Which of the three PATH situations the finished install is in. Split out from the printing so it
# can be tested: the caller supplies both PATH values, and every state is reachable without
# touching the machine's real environment.
#
#   ready         - the directory is on this process's PATH, so `rozi` resolves right now
#   stale-session - persisted for new terminals, but this one started before it was added
#   absent        - not on either, so nothing will find the command by name
function Get-CommandHintState([string]$ManagedBin, [string]$SessionPath, [string]$UserPath) {
    if (Test-PathContainsDirectory $SessionPath $ManagedBin) {
        return 'ready'
    }
    if (Test-PathContainsDirectory $UserPath $ManagedBin) {
        return 'stale-session'
    }
    return 'absent'
}

# `rozi` as a bare word only resolves if the managed directory is on PATH, and this script does not
# put it there unless asked. Printing `$ rozi` unconditionally handed a user whose PATH does not
# carry it a command that cannot be found, with nothing to say why.
#
# The remediation is the PowerShell that changes PATH, not a re-run of this installer with
# `-AddToPath`. Re-running re-downloads the archive and re-verifies its checksum and signature to
# append one string to the registry, and it does that work *after* the payload probe: on a machine
# whose application-control policy refuses the payload, the re-run fails before it reaches the PATH
# code at all. `-AddToPath` remains the right answer at install time, where it costs nothing extra.
#
# Both snippets check before they write, because an installer hint is something people paste twice.
# The PowerShell that puts the managed directory on PATH, as lines ready to print.
#
# Returned rather than printed so the tests can check the thing a user actually pastes. Every block
# defines every variable it uses: the two were previously offered as a pair that shared a `$bin`
# from the first, so anyone who needed only the second - the common case, a terminal that is one
# entry behind - pasted a snippet that failed on an undefined variable.
#
# Each write is guarded by the check above it, so a block is safe to run more than once.
function Get-PathRemediation([string]$State) {
    $lines = @('$bin = "$env:LOCALAPPDATA\rozi\bin"')
    if ($State -eq 'absent') {
        $lines += '$user = [Environment]::GetEnvironmentVariable(''Path'', ''User'')'
        $lines += 'if (($user -split '';'').TrimEnd(''\'') -notcontains $bin) {'
        $lines += '    [Environment]::SetEnvironmentVariable(''Path'', "$user;$bin".Trim('';''), ''User'')'
        $lines += '}'
    }
    $lines += 'if (($env:Path -split '';'').TrimEnd(''\'') -notcontains $bin) {'
    $lines += '    $env:Path += ";$bin"'
    $lines += '}'
    $lines
}

# `rozi` as a bare word only resolves if the managed directory is on PATH, and this script does not
# put it there unless asked. Printing `$ rozi` unconditionally handed a user whose PATH does not
# carry it a command that cannot be found, with nothing to say why.
#
# The remediation is the PowerShell that changes PATH, not a re-run of this installer with
# `-AddToPath`. Re-running re-downloads the archive and re-verifies its checksum and signature to
# append one string to the registry, and it does that after the payload probe: on a machine whose
# application-control policy refuses the payload, the re-run fails before it reaches the PATH code
# at all. `-AddToPath` remains the right answer at install time, where it costs nothing extra.
function Write-CommandHint([string]$ManagedBin) {
    $state = if ($ManagedBin) {
        Get-CommandHintState `
            $ManagedBin `
            ([Environment]::GetEnvironmentVariable('Path', 'Process')) `
            ([Environment]::GetEnvironmentVariable('Path', 'User'))
    } else {
        'ready'
    }

    if ($state -eq 'ready') {
        Write-Host '    Start it with:'
        Write-Host "      $($script:CAccent)rozi$($script:CReset)"
        return
    }

    if ($state -eq 'stale-session') {
        Write-Host " $($script:CWarn)!$($script:CReset)  On PATH for new terminals, but not this one. Run it with the full path:"
    } else {
        Write-Host " $($script:CWarn)!$($script:CReset)  Not on PATH yet. Run it with the full path:"
    }
    Write-Host "      $($script:CAccent)$(Join-Path $ManagedBin 'rozi.exe')$($script:CReset)"
    Write-Host ''
    if ($state -eq 'stale-session') {
        Write-Host '    or add it to this terminal (safe to run more than once):'
    } else {
        Write-Host '    or add it to PATH (safe to run more than once):'
    }
    foreach ($line in Get-PathRemediation $state) {
        Write-Host "      $($script:CViolet)$line$($script:CReset)"
    }
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
        Write-Done 'Download' (Format-Bytes (Get-Item -LiteralPath $archive).Length)
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
        # The one line worth catching an eye on: the only green in the run, with the version
        # painted in the wordmark's own gradient so the result reads as the same object the banner
        # announced.
        Write-Host " $($script:COk)$($script:GlyphOk)$($script:CReset)  $(Format-Gradient "Installed $ResolvedVersion")"
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
    # The Resolve row is still active and its thread is still turning: erasing the line and writing
    # the header underneath a live spinner puts the frame and the header on the same row, which is
    # what `Resolve   latest release rozi 0.0.11 - x86_64-...` was. Any main-thread write while a
    # row is spinning has to stop it first.
    Stop-Spinner
    if ($script:Interactive) { Write-Host -NoNewline "`r$($script:Esc)[2K" }
    Write-Host " $(Format-Gradient "rozi $Version")  $($script:CDim)$($script:GlyphSep)  $($script:CViolet)$target$($script:CReset)"
    Write-Host ''
    Write-Done 'Resolve' $resolvedDetail
    Install-Version $Version ([bool]$AddToPath)
    $script:ExitCode = 0
} catch {
    # Unconditionally, before anything else prints: an exception can land while a step is active,
    # and a spinner thread still turning would write its next frame over the failure.
    Stop-Spinner
    if ($script:CurrentOperation) {
        Write-Failed $script:CurrentOperation 'failed'
    }
    [Console]::Error.WriteLine('')
    [Console]::Error.WriteLine('installation failed')
    [Console]::Error.WriteLine($_.Exception.Message)
}

Exit-Installer
