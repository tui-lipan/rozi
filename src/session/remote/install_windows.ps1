param([string]$upload, [string]$version, [uint32]$min, [uint32]$max, [hashtable]$hashes)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System.Text;
using System.Runtime.InteropServices;
public static class RoziLongPathName {
  [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
  public static extern uint GetLongPathName(string shortPath, StringBuilder longPath, uint bufferLength);
}
'@ -ErrorAction SilentlyContinue
function Get-RoziLongPath($path) {
  $buffer = New-Object System.Text.StringBuilder 32768
  $length = [RoziLongPathName]::GetLongPathName($path, $buffer, [uint32]$buffer.Capacity)
  if ($length -gt 0 -and $length -lt $buffer.Capacity) { return $buffer.ToString() }
  return $path
}
$dataHome = $env:LOCALAPPDATA
if (-not $dataHome) { $dataHome = Join-Path $env:USERPROFILE '.local\share' }
$root = Join-Path $dataHome 'rozi\remote\versions'
$dir = Join-Path $root $version
$staging = Join-Path $dataHome ".$upload.stage"
$source = Join-Path $env:USERPROFILE $upload
$final = Join-Path $dir 'rozi.exe'
function Assert-Regular($path) {
  $item = Get-Item -Force -LiteralPath $path
  if (-not ($item -is [System.IO.FileInfo]) -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
    throw "refuse_non_regular=$path"
  }
}
function Assert-Directory($path) {
  $item = Get-Item -Force -LiteralPath $path
  if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
    throw "refuse_non_directory=$path"
  }
}
function Assert-Hash($path, $expected) {
  Assert-Regular $path
  if ((Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash -ne $expected) {
    throw "SHA-256 mismatch: $path"
  }
}
Assert-Directory $source
$stagingOwned = $false
try {
  New-Item -ItemType Directory -Force -Path $root | Out-Null
  Assert-Directory $root
  New-Item -ItemType Directory -Path $staging | Out-Null
  $stagingOwned = $true
  foreach ($name in $hashes.Keys) {
    $file = Join-Path $source $name
    Assert-Regular $file
    $staged = Join-Path $staging $name
    Copy-Item -LiteralPath $file -Destination $staged
    Assert-Hash $staged $hashes[$name]
  }
  $src = Join-Path $staging 'rozi.exe'
  $out = & $src --version 2>$null
  if ($LASTEXITCODE -ne 0) { throw "staged binary failed: $src" }
  $protocolMin = $null
  $protocolMax = $null
  foreach ($line in $out) {
    if ($line -match '^protocol_min=([0-9]+)$') { $protocolMin = [uint32]$Matches[1] }
    if ($line -match '^protocol_max=([0-9]+)$') { $protocolMax = [uint32]$Matches[1] }
  }
  if ($null -eq $protocolMin -or $null -eq $protocolMax) { throw "staged binary has no protocol range: $src" }
  if ($protocolMax -lt $min -or $protocolMin -gt $max) {
    throw "staged binary protocol mismatch: $protocolMin..$protocolMax"
  }
  $help = & $src --help 2>$null
  if ($LASTEXITCODE -ne 0 -or -not ($help -match '--remote')) { throw "staged binary has no remote support: $src" }
  if (-not (Test-Path -LiteralPath $dir)) {
    [IO.Directory]::Move($staging, $dir)
  } else {
    Assert-Directory $dir
    if ($hashes.Count -gt 1) {
      # A concurrent install may have won the directory rename. Reuse that exact bundle only.
      foreach ($name in $hashes.Keys) {
        Assert-Hash (Join-Path $dir $name) $hashes[$name]
      }
    } else {
      # Keep the explicit binary override's existing ability to replace a bare executable.
      if (Test-Path -LiteralPath $final) { Assert-Regular $final }
      if (Get-ChildItem -Force -LiteralPath $dir | Where-Object { $_.Name -ne 'rozi.exe' }) {
        throw "refuse_to_replace_bundle_with_bare_payload=$dir"
      }
      Move-Item -Force -LiteralPath $src -Destination $final
    }
  }
  $installed = Get-RoziLongPath (Resolve-Path -LiteralPath $final).Path
  Write-Output "installed=$installed"
} finally {
  if ($stagingOwned -and (Test-Path -LiteralPath $staging)) { Remove-Item -Recurse -Force -LiteralPath $staging }
  if (Test-Path -LiteralPath $source) { Remove-Item -Recurse -Force -LiteralPath $source }
}
