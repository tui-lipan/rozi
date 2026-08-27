#!/usr/bin/env bash
set -euo pipefail

# Bootstrap helper for Unix hosts; managed installation is delegated to the extracted release
# payload. It never edits a shell startup file.
# Trust-boundary caveat: "Downloading an archive and its checksum from the same HTTPS release location protects against corruption, but does not provide independent authenticity if the release account or release assets are compromised."

readonly RELEASE_REPO="${ROZI_RELEASE_REPO:-tui-lipan/rozi}"
readonly DEFAULT_LATEST_URL="https://github.com/${RELEASE_REPO}/releases/latest"
readonly MAX_ARCHIVE_BYTES=268435456
readonly MAX_CHECKSUM_BYTES=1048576
readonly MAX_LISTING_BYTES=16777216
readonly MAX_ARCHIVE_MEMBERS=10000
readonly CAVEAT='Downloading an archive and its checksum from the same HTTPS release location protects against corruption, but does not provide independent authenticity if the release account or release assets are compromised.'

# A terminal gets one rewritten active row. Colour is a separate choice so NO_COLOR keeps the
# compact interaction without escape-coded styling. Redirected and CI output remains a normal,
# append-only transcript.
if [[ -t 1 && "${TERM:-dumb}" != "dumb" ]]; then
  INTERACTIVE=1
else
  INTERACTIVE=0
fi
if ((INTERACTIVE)) && [[ -z "${NO_COLOR:-}" ]]; then
  # The rozi palette, matching `platform::ansi::palette` and the logo's rose-to-violet gradient.
  C_RESET=$'\033[0m'
  C_DIM=$'\033[38;2;142;147;180m'
  C_ACCENT=$'\033[38;2;253;74;128m'
  C_VIOLET=$'\033[38;2;152;43;242m'
  # The unfilled remainder of the meter. Near the app's border colour so a track reads as chrome
  # rather than as data - painting it in the accent hides where the fill actually ends.
  C_TRACK=$'\033[38;2;52;56;88m'
  C_OK=$'\033[38;2;74;222;128m'
  C_ERROR=$'\033[38;2;255;95;87m'
else
  C_RESET=''
  C_DIM=''
  C_ACCENT=''
  C_VIOLET=''
  C_TRACK=''
  C_OK=''
  C_ERROR=''
fi
readonly INTERACTIVE C_RESET C_DIM C_ACCENT C_OK C_ERROR
CURRENT_OPERATION=''

# Deliberately ASCII. A Windows console under a non-UTF-8 code page mangles box-drawing and block
# characters, and this wordmark has a PowerShell twin that must look identical.
banner() {
  local line column band painted previous width=18
  local -a art=(
    '                _ '
    '  _ __ ___ ___ (_)'
    " | '__/ _ \_  /| |"
    ' | | | (_) / / | |'
    ' |_|  \___/___||_|'
  )
  # The same rose-to-violet gradient the download meter draws, sampled in four bands across the
  # width. One escape per band rather than per character, which at this width still reads as a
  # gradient. Hardcoded like the meter's, because the palette above carries only the two ends.
  local -a bands=(
    $'\033[38;2;253;74;128m'
    $'\033[38;2;228;66;156m'
    $'\033[38;2;203;58;185m'
    $'\033[38;2;178;51;213m'
  )
  for line in "${art[@]}"; do
    if [[ -z "$C_ACCENT" ]]; then
      printf '%s\n' "$line"
      continue
    fi
    painted=''
    previous=-1
    for ((column = 0; column < ${#line}; column++)); do
      band=$((column * 4 / width))
      ((band > 3)) && band=3
      if ((band != previous)); then
        painted+="${bands[band]}"
        previous=$band
      fi
      painted+="${line:column:1}"
    done
    printf '%s%s\n' "$painted" "$C_RESET"
  done
  printf '\n'
}

status_row() {
  local symbol="$1" color="$2" operation="$3" detail="$4"
  if ((INTERACTIVE)); then
    printf '\r\033[2K  %s%s%s %-12s%s' "$color" "$symbol" "$C_RESET" "$operation" "$detail"
  else
    printf '  %s %-12s%s\n' "$symbol" "$operation" "$detail"
  fi
}

status_active() {
  CURRENT_OPERATION="$1"
  status_row '●' "$C_ACCENT" "$1" "$2"
}

status_done() {
  status_row '✓' "$C_OK" "$1" "$2"
  ((INTERACTIVE)) && printf '\n'
  CURRENT_OPERATION=''
}

status_failed() {
  status_row '✗' "$C_ERROR" "$1" "$2"
  ((INTERACTIVE)) && printf '\n'
  CURRENT_OPERATION=''
}

fail() {
  if [[ -n "$CURRENT_OPERATION" ]]; then
    status_failed "$CURRENT_OPERATION" 'failed'
  fi
  printf '\ninstallation failed\n' >&2
  printf '%s\n' "$1" >&2
  exit 1
}

usage_error() {
  printf 'rozi install: %s\n' "$1" >&2
  printf 'Use --help for usage.\n' >&2
  exit 2
}

usage() {
  cat <<EOF
Usage:
  install.sh [--version VERSION]

The default version is the current GitHub release. --version selects an exact release archive.
ROZI_RELEASE_BASE_URL may point at an HTTPS release mirror, and ROZI_RELEASE_LATEST_URL
selects an HTTPS /releases/latest redirect endpoint whose final URL must contain a v-prefixed tag.

After bootstrap verification, this script executes the extracted payload with \`install\`. The
installed CLI owns the managed versions, active pointer, launcher, rollback metadata, and command
path; this script does not create any of those files or edit a shell startup file.

Use the installed command for lifecycle operations:
  rozi update --check
  rozi update
  rozi update --rollback

Trust-boundary caveat: "$CAVEAT"

Exit status: 0 means success; 1 means download, checksum, archive, or install failure; 2 means
invalid command-line usage.
EOF
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

validate_version() {
  [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.+][0-9A-Za-z.-]+)?$ ]] ||
    usage_error "invalid release version: $1"
}

normalize_hex() {
  printf '%s' "$1" | tr '[:upper:]' '[:lower:]'
}

file_size() {
  if stat -c '%s' "$1" >/dev/null 2>&1; then
    stat -c '%s' "$1"
  else
    stat -f '%z' "$1"
  fi
}

# `show_progress` is for the one download worth watching: the release archive is several megabytes
# and was previously silent long enough to look hung. The checksum beside it is under a kilobyte and
# would only flash a bar on and off.
download_file() {
  local url="$1" destination="$2" max_bytes="$3" label="$4" show_progress="${5:-0}" size
  local expected=0 curl_error="${destination}.curl-error" curl_pid rc=0
  if ((INTERACTIVE && show_progress)); then
    expected="$(curl --fail --silent --location --head --proto '=https' --proto-redir '=https' \
      --max-redirs 5 "$url" 2>/dev/null | awk 'tolower($1)=="content-length:" { value=$2 } END { gsub("\r", "", value); print value+0 }')"
  fi
  curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
    --max-redirs 5 --max-filesize "$max_bytes" --output "$destination" "$url" 2>"$curl_error" &
  curl_pid=$!
  if ((INTERACTIVE && show_progress)); then
    while kill -0 "$curl_pid" 2>/dev/null; do
      if ((expected > 0)) && [[ -f "$destination" ]]; then
        render_download_progress "$(file_size "$destination" 2>/dev/null || printf 0)" "$expected"
      fi
      sleep 0.1
    done
  fi
  wait "$curl_pid" || rc=$?
  if ((rc != 0)); then
    local detail
    detail="$(tr '\n' ' ' <"$curl_error")"
    fail "could not download $label${detail:+: $detail}"
  fi
  rm -f "$curl_error"
  size="$(file_size "$destination")" || fail "could not stat downloaded $label"
  [[ "$size" =~ ^[0-9]+$ && "$size" -le "$max_bytes" ]] ||
    fail "downloaded $label exceeds its size limit"
}

# The meter draws its filled run and its track in different weights *and* different colours: a
# heavy glyph for what has arrived, a light one for what has not. The weight difference is what
# survives NO_COLOR, and the colour difference is what makes the boundary obvious at a glance.
#
# The filled run carries the logo gradient, stepped from rose to violet across the track. Bash
# cannot interpolate per cell cheaply, so it is sampled in bands - close enough at this width to
# read as a gradient, and it costs one escape per band rather than one per cell.
render_download_progress() {
  local current="$1" total="$2" width=32 percent filled empty bar='' cell band
  ((total > 0)) || return 0
  percent=$((current * 100 / total))
  ((percent > 100)) && percent=100
  # Reserve the last cell until the download is actually complete: a meter that reads full while
  # bytes are still arriving is worse than one that reaches the end a moment late.
  filled=$((percent * width / 100))
  ((percent < 100 && filled >= width)) && filled=$((width - 1))
  empty=$((width - filled))
  if ((filled > 0)); then
    if [[ -n "$C_ACCENT" ]]; then
      for ((cell = 0; cell < filled; cell++)); do
        # Four bands across the track, stepping rose -> violet.
        band=$((cell * 4 / width))
        case "$band" in
          0) bar+=$'\033[38;2;253;74;128m━' ;;
          1) bar+=$'\033[38;2;228;66;156m━' ;;
          2) bar+=$'\033[38;2;203;58;185m━' ;;
          *) bar+=$'\033[38;2;178;51;213m━' ;;
        esac
      done
    else
      printf -v bar '%*s' "$filled" '' && bar="${bar// /━}"
    fi
  fi
  if ((empty > 0)); then
    local tail
    printf -v tail '%*s' "$empty" ''
    bar+="${C_TRACK}${tail// /─}"
  fi
  status_row '●' "$C_ACCENT" 'Download' "${bar}${C_RESET} ${percent}%"
}

resolve_latest_version() {
  local latest_url location tag latest_path
  latest_url="${ROZI_RELEASE_LATEST_URL:-$DEFAULT_LATEST_URL}"
  [[ "$latest_url" == https://* ]] || fail "latest-release URL must use HTTPS"
  latest_path="${latest_url%%\?*}"
  latest_path="${latest_path%%\#*}"
  latest_path="${latest_path%/}"
  [[ "$latest_path" == */releases/latest ]] ||
    fail 'latest-release URL must end in /releases/latest'
  location="$(curl --fail --silent --show-error --location \
    --proto '=https' --proto-redir '=https' --max-redirs 5 \
    --output /dev/null --write-out '%{url_effective}' "$latest_url")" ||
    fail "could not resolve the current released version"
  location="${location%/}"
  tag="${location##*/}"
  [[ "$tag" == v* ]] || fail "latest-release URL did not resolve to a v-prefixed tag"
  printf '%s\n' "${tag#v}"
}

target_triple() {
  local os machine
  os="$(uname -s)"
  machine="$(uname -m)"
  case "$os:$machine" in
    Linux:x86_64) printf '%s\n' 'x86_64-unknown-linux-gnu' ;;
    Linux:aarch64|Linux:arm64) printf '%s\n' 'aarch64-unknown-linux-gnu' ;;
    Darwin:x86_64) printf '%s\n' 'x86_64-apple-darwin' ;;
    Darwin:arm64|Darwin:aarch64) printf '%s\n' 'aarch64-apple-darwin' ;;
    *) fail "unsupported host platform: $os/$machine" ;;
  esac
}

verify_checksum() {
  local archive checksum expected listed actual computed checksum_size
  archive="$1"
  checksum="$2"
  expected="$(normalize_hex "$3")"
  checksum_size="$(file_size "$checksum")" || fail "could not stat checksum $checksum"
  [[ "$checksum_size" =~ ^[0-9]+$ && "$checksum_size" -le "$MAX_CHECKSUM_BYTES" ]] ||
    fail "checksum exceeds its size limit: $checksum"
  listed="$(awk 'NF { print; exit }' "$checksum")" || fail "cannot read checksum $checksum"
  [[ -n "$listed" ]] || fail "checksum file is empty: $checksum"
  read -r actual listed_name extra <<<"$listed"
  listed_name="${listed_name#\*}"
  [[ -z "${extra:-}" && "$actual" =~ ^[0-9A-Fa-f]{64}$ ]] ||
    fail "malformed checksum line in $checksum"
  [[ "$listed_name" == "$(basename "$archive")" ]] ||
    fail "checksum names a different archive: $listed_name"
  if command -v sha256sum >/dev/null 2>&1; then
    computed="$(sha256sum "$archive" | awk '{ print $1 }')"
  elif command -v shasum >/dev/null 2>&1; then
    computed="$(shasum -a 256 "$archive" | awk '{ print $1 }')"
  else
    fail 'sha256sum or shasum is required to verify the archive'
  fi
  computed="$(normalize_hex "$computed")"
  actual="$(normalize_hex "$actual")"
  [[ "$computed" == "$actual" ]] || fail "archive checksum does not match $checksum"
  [[ "$computed" == "$expected" ]] || fail "checksum changed while it was being read"
}

validate_tar_members() {
  local archive="$1" stem="$2" scratch="$3"
  local names types normalized_names member normalized line payload_member member_count
  local listing listing_limit_kib duplicate listing_size
  local payload_count=0
  names="$scratch/tar-members"
  types="$scratch/tar-types"
  normalized_names="$scratch/tar-members-normalized"
  listing_limit_kib=$((MAX_LISTING_BYTES / 1024))
  if ! (ulimit -f "$listing_limit_kib"; tar -tzf "$archive" >"$names"); then
    fail "archive member listing exceeds its limit: $archive"
  fi
  if ! (ulimit -f "$listing_limit_kib"; tar -tvzf "$archive" >"$types"); then
    fail "archive type listing exceeds its limit: $archive"
  fi
  for listing in "$names" "$types"; do
    listing_size="$(file_size "$listing")" || fail "could not stat archive listing: $listing"
    [[ "$listing_size" =~ ^[0-9]+$ && "$listing_size" -le "$MAX_LISTING_BYTES" ]] ||
      fail "archive listing exceeds its size limit: $listing"
  done
  payload_member="$stem/rozi"
  member_count=0
  exec 3<"$types"
  while IFS= read -r member || [[ -n "$member" ]]; do
    if ! IFS= read -r line <&3; then
      exec 3<&-
      fail 'archive member listings disagree; refusing to extract'
    fi
    member_count=$((member_count + 1))
    [[ "$member_count" -le "$MAX_ARCHIVE_MEMBERS" ]] ||
      fail "archive contains more than $MAX_ARCHIVE_MEMBERS members"
    [[ -n "$member" ]] || fail 'archive contains an empty member name'
    normalized="${member%/}"
    [[ -n "$normalized" ]] || fail "unsafe path in release archive: $member"
    case "$member" in
      *\\*|*//*) fail "unsafe path in release archive: $member" ;;
    esac
    case "/$normalized/" in
      */../*|*/./*) fail "unsafe path in release archive: $member" ;;
    esac
    case "$normalized" in
      "$stem"|"$stem"/*) ;;
      *) fail "archive member escapes canonical root: $member" ;;
    esac
    case "${line:0:1}" in
      -|d) ;;
      *) fail "archive contains a link or special member: $line" ;;
    esac
    printf '%s\n' "$normalized" >>"$normalized_names" ||
      fail 'could not record normalized archive member names'
    if [[ "$member" == "$payload_member" ]]; then
      [[ "${line:0:1}" == '-' ]] ||
        fail "canonical payload is not a regular file: $payload_member"
      payload_count=$((payload_count + 1))
    fi
  done <"$names"
  if IFS= read -r line <&3; then
    exec 3<&-
    fail 'archive member listings disagree; refusing to extract'
  fi
  exec 3<&-
  [[ "$payload_count" -eq 1 ]] ||
    fail "archive must contain exactly one regular payload: $payload_member"

  duplicate="$(LC_ALL=C sort "$names" | uniq -d | awk 'NF && !found { print; found=1 }')"
  [[ -z "$duplicate" ]] || fail "archive contains duplicate member: $duplicate"
  duplicate="$(LC_ALL=C sort "$normalized_names" | uniq -d | awk 'NF && !found { print; found=1 }')"
  [[ -z "$duplicate" ]] || fail "archive contains duplicate normalized member: $duplicate"

}

managed_cli() {
  local payload="$1" help output
  [[ -x "$payload" ]] || fail "archive payload is not executable: $payload"
  help="$("$payload" --help 2>&1)" ||
    fail "verified archive payload could not print --help: $payload"
  grep -Eq '(^|[[:space:]])install([[:space:]]|$)' <<<"$help" ||
    fail "verified archive payload has no 'install' command; no managed files were changed"
  if ! output="$("$payload" install 2>&1)"; then
    output="${output#rozi: installation failed: }"
    if [[ "$output" == *'release verification error'* || "$output" == *certificate* || "$output" == *signature* ]]; then
      status_failed 'Signature' 'verification failed'
    else
      status_failed 'Install' 'activation failed'
    fi
    printf '\ninstallation failed\n' >&2
    printf '%s\n' "$output" >&2
    exit 1
  fi
  status_done 'Signature' 'Ed25519 verified'
  status_active 'Install' 'activating command'
  status_done 'Install' '~/.local/bin/rozi'
}

# `rozi` as a bare word only resolves if the managed directory is on PATH, and this script
# deliberately does not edit a shell startup file. Printing `$ rozi` unconditionally handed a user
# whose PATH does not carry it a command that cannot be found, with nothing to say why.
#
# Unlike the PowerShell installer there is no persisted-versus-current distinction to report here:
# a shell's PATH comes from startup files this script neither reads nor writes, so `$PATH` is both
# the only thing it can inspect and the only thing that matters for the command just printed.
command_hint() {
  local bin="$HOME/.local/bin"
  case ":$PATH:" in
    *":$bin:"*)
      printf '  %s$ rozi%s\n' "$C_DIM" "$C_RESET"
      return 0
      ;;
  esac
  printf '  rozi is not on your PATH yet.\n'
  printf '\n'
  printf '  Run it now:\n'
  printf '    %s%s/rozi%s\n' "$C_ACCENT" "$bin" "$C_RESET"
  printf '\n'
  printf '  Or add it to PATH - put this in your shell profile:\n'
  printf '    %s%s%s\n' "$C_DIM" 'export PATH="$HOME/.local/bin:$PATH"' "$C_RESET"
}

install_version() {
  local version="$1" target="$2" base="$3"
  local archive_name stem archive checksum temp_extract payload payload_limit_kib
  archive_name="rozi-${version}-${target}.tar.gz"
  stem="${archive_name%.tar.gz}"
  base="${base%/}"
  [[ "$base" == https://* ]] || fail 'release base URL must use HTTPS'

  temp_extract="$(mktemp -d "${TMPDIR:-/tmp}/rozi-install.XXXXXX")"
  trap 'rm -rf -- "${temp_extract:-}"' EXIT
  archive="$temp_extract/$archive_name"
  checksum="$archive.sha256"
  status_active 'Download' "$archive_name"
  download_file "$base/$archive_name" "$archive" "$MAX_ARCHIVE_BYTES" "$archive_name" 1
  download_file "$base/$archive_name.sha256" "$checksum" "$MAX_CHECKSUM_BYTES" \
    "adjacent checksum for $archive_name"
  status_done 'Download' "$archive_name"

  # The checksum detects corruption, not authenticity.  The extracted payload performs signed
  # metadata verification before activation, but the bootstrap payload is already executing.
  status_active 'Checksum' 'verifying SHA-256'
  verify_checksum "$archive" "$checksum" "$(awk 'NF { print $1; exit }' "$checksum")"
  status_done 'Checksum' 'SHA-256 verified'
  validate_tar_members "$archive" "$stem" "$temp_extract"
  local payload_size
  payload="$(mktemp "$temp_extract/payload.XXXXXX")" ||
    fail 'could not create a temporary payload file'
  payload_limit_kib=$((MAX_ARCHIVE_BYTES / 1024))
  if ! (ulimit -f "$payload_limit_kib"; tar --no-recursion -xOzf "$archive" "$stem/rozi" >"$payload")
  then
    fail "could not extract canonical payload within its size limit: $stem/rozi"
  fi
  chmod 700 "$payload" || fail "could not mark temporary payload executable: $payload"
  [[ -f "$payload" && ! -L "$payload" && -x "$payload" ]] ||
    fail "temporary payload is not an executable regular file: $payload"
  payload_size="$(file_size "$payload")" || fail "could not stat extracted payload: $payload"
  [[ "$payload_size" =~ ^[0-9]+$ && "$payload_size" -le "$MAX_ARCHIVE_BYTES" ]] ||
    fail "extracted payload exceeds its size limit: $payload"
  local version_output version_line
  status_active 'Signature' 'verifying signed release'
  if ! version_output="$("$payload" --version 2>&1)"; then
    fail "archive payload could not report its version${version_output:+: $version_output}"
  fi
  version_line="${version_output%%$'\n'*}"
  [[ "$version_line" == "rozi $version" ]] ||
    fail "archive payload version line is not exactly: rozi $version"
  managed_cli "$payload"

  printf '\n'
  printf '  rozi %s installed successfully\n' "$version"
  printf '\n'
  command_hint
  printf '\n'
  trap - EXIT
  rm -rf "$temp_extract"
}

main() {
  local version='' target base resolved_detail
  while (($#)); do
    case "$1" in
      --version)
        (($# >= 2)) || usage_error '--version requires a value'
        version="$2"
        shift
        ;;
      --help|-h)
        usage
        return 0
        ;;
      --*) usage_error "unknown option: $1" ;;
      *) usage_error "unexpected argument: $1" ;;
    esac
    shift
  done

  require_command curl
  require_command tar
  require_command mktemp
  require_command sort
  require_command uniq
  banner
  if [[ -z "$version" ]]; then
    status_active 'Resolve' 'latest release'
    version="$(resolve_latest_version)"
    resolved_detail="latest release $version"
  else
    resolved_detail="release $version"
  fi
  validate_version "$version"
  target="$(target_triple)"
  if ((INTERACTIVE)); then
    printf '\r\033[2K'
  fi
  printf '  %srozi %s  ·  %s%s\n\n' "$C_DIM" "$version" "$target" "$C_RESET"
  status_done 'Resolve' "$resolved_detail"
  base="${ROZI_RELEASE_BASE_URL:-https://github.com/${RELEASE_REPO}/releases/download/v${version}}"
  install_version "$version" "$target" "$base"
}

main "$@"
