#!/usr/bin/env bash
set -euo pipefail

# Bootstrap helper for Unix hosts; managed installation is delegated to the extracted release
# payload. It never edits a shell startup file.
# Trust-boundary caveat: "Downloading an archive and its checksum from the same HTTPS release location protects against corruption, but does not provide independent authenticity if the release account or release assets are compromised."

readonly RELEASE_REPO="${ROZI_RELEASE_REPO:-Razuer/hyprmux}"
readonly DEFAULT_LATEST_URL="https://github.com/${RELEASE_REPO}/releases/latest"
readonly MAX_ARCHIVE_BYTES=268435456
readonly MAX_CHECKSUM_BYTES=1048576
readonly MAX_LISTING_BYTES=16777216
readonly MAX_ARCHIVE_MEMBERS=10000
readonly CAVEAT='Downloading an archive and its checksum from the same HTTPS release location protects against corruption, but does not provide independent authenticity if the release account or release assets are compromised.'

fail() {
  printf 'hyprmux install: %s\n' "$1" >&2
  exit 1
}

usage_error() {
  printf 'hyprmux install: %s\n' "$1" >&2
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
  hyprmux update --check
  hyprmux update
  hyprmux update --rollback

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

download_file() {
  local url="$1" destination="$2" max_bytes="$3" label="$4" size
  curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
    --max-redirs 5 --max-filesize "$max_bytes" --output "$destination" "$url" ||
    fail "could not download $label"
  size="$(file_size "$destination")" || fail "could not stat downloaded $label"
  [[ "$size" =~ ^[0-9]+$ && "$size" -le "$max_bytes" ]] ||
    fail "downloaded $label exceeds its size limit"
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
  payload_member="$stem/hyprmux"
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
  local payload="$1" help
  [[ -x "$payload" ]] || fail "archive payload is not executable: $payload"
  help="$("$payload" --help 2>&1)" ||
    fail "verified archive payload could not print --help: $payload"
  grep -Eq '(^|[[:space:]])install([[:space:]]|$)' <<<"$help" ||
    fail "verified archive payload has no 'install' command; no managed files were changed"
  "$payload" install || fail "managed installation failed; no bootstrap layout was created by this script"
}

install_version() {
  local version="$1" target="$2" base="$3"
  local archive_name stem archive checksum temp_extract payload payload_limit_kib
  archive_name="hyprmux-${version}-${target}.tar.gz"
  stem="${archive_name%.tar.gz}"
  base="${base%/}"
  [[ "$base" == https://* ]] || fail 'release base URL must use HTTPS'

  temp_extract="$(mktemp -d "${TMPDIR:-/tmp}/hyprmux-install.XXXXXX")"
  trap 'rm -rf -- "${temp_extract:-}"' EXIT
  archive="$temp_extract/$archive_name"
  checksum="$archive.sha256"
  download_file "$base/$archive_name" "$archive" "$MAX_ARCHIVE_BYTES" "$archive_name"
  download_file "$base/$archive_name.sha256" "$checksum" "$MAX_CHECKSUM_BYTES" \
    "adjacent checksum for $archive_name"

  # The checksum detects corruption, not authenticity.  The extracted payload performs signed
  # metadata verification before activation, but the bootstrap payload is already executing.
  verify_checksum "$archive" "$checksum" "$(awk 'NF { print $1; exit }' "$checksum")"
  validate_tar_members "$archive" "$stem" "$temp_extract"
  local payload_size
  payload="$(mktemp "$temp_extract/payload.XXXXXX")" ||
    fail 'could not create a temporary payload file'
  payload_limit_kib=$((MAX_ARCHIVE_BYTES / 1024))
  if ! (ulimit -f "$payload_limit_kib"; tar --no-recursion -xOzf "$archive" "$stem/hyprmux" >"$payload")
  then
    fail "could not extract canonical payload within its size limit: $stem/hyprmux"
  fi
  chmod 700 "$payload" || fail "could not mark temporary payload executable: $payload"
  [[ -f "$payload" && ! -L "$payload" && -x "$payload" ]] ||
    fail "temporary payload is not an executable regular file: $payload"
  payload_size="$(file_size "$payload")" || fail "could not stat extracted payload: $payload"
  [[ "$payload_size" =~ ^[0-9]+$ && "$payload_size" -le "$MAX_ARCHIVE_BYTES" ]] ||
    fail "extracted payload exceeds its size limit: $payload"
  local version_output version_line
  version_output="$("$payload" --version 2>&1)" ||
    fail "archive payload could not report its version"
  version_line="${version_output%%$'\n'*}"
  [[ "$version_line" == "hyprmux $version" ]] ||
    fail "archive payload version line is not exactly: hyprmux $version"

  managed_cli "$payload"
  printf 'installed hyprmux %s for %s through the extracted release payload\n' "$version" "$target"
  printf 'bootstrap caveat: %s\n' "$CAVEAT"
  trap - EXIT
  rm -rf "$temp_extract"
}

main() {
  local version='' target base
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
  if [[ -z "$version" ]]; then
    version="$(resolve_latest_version)"
  fi
  validate_version "$version"
  target="$(target_triple)"
  base="${ROZI_RELEASE_BASE_URL:-https://github.com/${RELEASE_REPO}/releases/download/v${version}}"
  install_version "$version" "$target" "$base"
}

main "$@"
