#!/bin/sh
# Installs mailtriage from its GitHub releases:
#
#   curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/wir-drei-digital/mailtriage/main/install.sh | sh
#   curl ... | sh -s -- [--version X.Y.Z] [--dir DIR] [--tray|--no-tray] [--no-setup] [--yes] [--uninstall]
#
# It downloads and verifies the release archive, then hands over to the
# downloaded binary's `mailtriage self install`, which installs it into DIR
# (default ~/.local/bin) and offers setup. It never uses sudo, never edits
# shell startup files and never touches a himalaya. Exit codes: 0 done;
# 1 a failed step; 2 invalid options, an unsupported platform or a refused
# downgrade; otherwise the exit code of `mailtriage self install`.
#
# The whole script after this comment is one brace group: sh reads all of
# it before running anything, so a download that broke off runs nothing.
{
set -eu

# The release this copy installs by default; the release workflow sets it
# in the copy attached to each release. Empty: the newest release.
default_version=''

repo_url='https://github.com/wir-drei-digital/mailtriage'
max_binary_bytes=209715200

say() {
  printf 'mailtriage install: %s\n' "$*" >&2
}

# A failed step: exit 1.
fail() {
  say "$1"
  exit 1
}

# Invalid options, an unsupported platform: exit 2.
refuse() {
  say "$1"
  exit 2
}

usage() {
  cat >&2 <<'USAGE'
usage: install.sh [--version X.Y.Z] [--dir DIR] [--tray|--no-tray] [--no-setup] [--yes] [--uninstall]

  --version X.Y.Z  install this release instead of the newest (MAILTRIAGE_VERSION)
  --dir DIR        install directory, default ~/.local/bin (MAILTRIAGE_INSTALL_DIR)
  --tray           install the tray app too (MAILTRIAGE_TRAY=1)
  --no-tray        do not install the tray app (MAILTRIAGE_TRAY=0)
  --no-setup       do not offer setup or the login item (MAILTRIAGE_NO_SETUP=1)
  --yes            never prompt (MAILTRIAGE_YES=1)
  --uninstall      uninstall the installation in DIR
USAGE
}

# X.Y.Z with decimal parts and no leading zeros.
is_version() {
  case "$1" in
    '' | *[!0-9.]* | .* | *. | *..*) return 1 ;;
  esac
  old_ifs=$IFS
  IFS=.
  # The value holds only digits and dots: splitting it is safe.
  # shellcheck disable=SC2086
  set -- $1
  IFS=$old_ifs
  [ "$#" -eq 3 ] || return 1
  for part in "$@"; do
    case "$part" in
      0?*) return 1 ;;
    esac
  done
}

# A boolean from the environment: empty, 0 or 1.
flag_value() {
  case "$2" in
    '' | 0 | 1) printf '%s' "$2" ;;
    *) refuse "$1 must be 0 or 1" ;;
  esac
}

need() {
  command -v "$1" >/dev/null 2>&1 || fail "missing tool: $1"
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

# GET $1 into the file $2 (or with $3 = -w, print the effective URL).
fetch() {
  curl --proto "$proto" --proto-redir "$proto" --tlsv1.2 -fsSL \
    --max-redirs 10 --connect-timeout 30 --max-time 600 "$@"
}

# Checks the archive $1 in $tmp against its single SHA256SUMS line.
verify() {
  matches=$(awk -v name="$1" '$2 == name' "$tmp/SHA256SUMS")
  count=$(printf '%s\n' "$matches" | awk 'NF' | wc -l | tr -d ' ')
  [ "$count" = 1 ] || fail "SHA256SUMS must have exactly one line for $1, not $count"
  expected=$(printf '%s\n' "$matches" | awk 'length($1) == 64 && $1 ~ /^[0-9a-f]+$/ && $0 == $1 "  " $2 { print $1 }')
  [ -n "$expected" ] || fail "SHA256SUMS has a malformed line for $1"
  [ "$(sha256 "$tmp/$1")" = "$expected" ] || fail "checksum mismatch for $1"
}

# The archive $1 in $tmp lists exactly the top-level regular files $2...,
# each once; then only the first of them is extracted into $tmp.
unpack() {
  archive=$1
  shift
  listing=$(tar -tzvf "$tmp/$archive") || fail "cannot read $archive"
  seen=' '
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    case "$line" in
      *' -> '* | *' link to '*) fail "$archive holds a link" ;;
      -*) ;;
      *) fail "$archive holds something other than a regular file" ;;
    esac
    name=${line##* }
    case " $* " in
      *" $name "*) ;;
      *) fail "$archive holds an unexpected entry: $name" ;;
    esac
    case "$seen" in
      *" $name "*) fail "$archive holds $name twice" ;;
    esac
    seen="$seen$name "
  done <<LISTING
$listing
LISTING
  for name in "$@"; do
    case "$seen" in
      *" $name "*) ;;
      *) fail "$archive has no $name" ;;
    esac
  done
  tar -xzf "$tmp/$archive" -C "$tmp" "$1" || fail "cannot extract $1 from $archive"
  if [ ! -f "$tmp/$1" ] || [ -L "$tmp/$1" ]; then
    fail "$1 in $archive is not a regular file"
  fi
  size=$(wc -c <"$tmp/$1" | tr -d ' ')
  [ "$size" -le "$max_binary_bytes" ] || fail "$1 in $archive is larger than 200 MB"
  chmod 0755 "$tmp/$1"
}

# The stdin for the hand-over: the terminal when it can be opened and
# --yes is not given, else /dev/null. The script itself arrives on stdin.
input() {
  if [ "$yes" != 1 ] && (: <"$tty") 2>/dev/null; then
    printf '%s' "$tty"
  else
    printf '%s' /dev/null
  fi
}

main() {
  version=${MAILTRIAGE_VERSION:-}
  dir=${MAILTRIAGE_INSTALL_DIR:-}
  tray=$(flag_value MAILTRIAGE_TRAY "${MAILTRIAGE_TRAY:-}")
  no_setup=$(flag_value MAILTRIAGE_NO_SETUP "${MAILTRIAGE_NO_SETUP:-}")
  yes=$(flag_value MAILTRIAGE_YES "${MAILTRIAGE_YES:-}")
  uninstall=
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --version)
        [ "$#" -ge 2 ] || refuse "--version needs X.Y.Z"
        version=$2
        shift 2
        ;;
      --version=*)
        version=${1#--version=}
        shift
        ;;
      --dir)
        [ "$#" -ge 2 ] || refuse "--dir needs a directory"
        dir=$2
        shift 2
        ;;
      --dir=*)
        dir=${1#--dir=}
        shift
        ;;
      --tray)
        tray=1
        shift
        ;;
      --no-tray)
        tray=0
        shift
        ;;
      --no-setup)
        no_setup=1
        shift
        ;;
      --yes)
        yes=1
        shift
        ;;
      --uninstall)
        uninstall=1
        shift
        ;;
      -h | --help)
        usage
        exit 0
        ;;
      *)
        usage
        refuse "unknown option: $1"
        ;;
    esac
  done
  # A literal ~ (a quoted --dir or MAILTRIAGE_INSTALL_DIR) means HOME; the
  # patterns are quoted, so the shell does not expand them itself (hence
  # SC2088 is off here).
  # shellcheck disable=SC2088
  case "$dir" in
    '~' | '~/'*)
      [ -n "${HOME:-}" ] || refuse "HOME is not set; pass --dir without ~"
      dir="$HOME${dir#?}"
      ;;
  esac
  if [ -z "$dir" ]; then
    [ -n "${HOME:-}" ] || refuse "HOME is not set; pass --dir"
    dir="$HOME/.local/bin"
  fi
  [ -z "$version" ] || is_version "$version" || refuse "--version must be X.Y.Z, not $version"

  # Tests replace GitHub with a loopback server; only then is plain HTTP
  # allowed, and only then is the terminal replaceable.
  base=$repo_url
  proto='=https'
  tty=/dev/tty
  if [ -n "${MAILTRIAGE_INSTALL_URL:-}" ]; then
    case "$MAILTRIAGE_INSTALL_URL" in
      *@*) refuse "MAILTRIAGE_INSTALL_URL must be a loopback URL" ;;
      http://127.0.0.1 | http://127.0.0.1:* | http://127.0.0.1/*) ;;
      http://localhost | http://localhost:* | http://localhost/*) ;;
      *) refuse "MAILTRIAGE_INSTALL_URL must be a loopback URL" ;;
    esac
    base=${MAILTRIAGE_INSTALL_URL%/}
    proto='=http'
    tty=${MAILTRIAGE_INSTALL_TTY:-/dev/tty}
  fi

  if [ "$uninstall" = 1 ]; then
    [ -f "$dir/mailtriage" ] || fail "nothing is installed in $dir"
    set -- self uninstall --dir "$dir"
    if [ "$yes" = 1 ]; then set -- "$@" --yes; fi
    status=0
    "$dir/mailtriage" "$@" <"$(input)" || status=$?
    if [ "$status" = 126 ]; then
      say "could not run $dir/mailtriage (exit 126); $dir may be mounted noexec, or the file is not executable"
    fi
    exit "$status"
  fi

  need curl
  need tar
  need mktemp
  need uname
  command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1 ||
    fail "missing tool: sha256sum or shasum"
  os=$(uname -s)
  arch=$(uname -m)
  case "$os $arch" in
    'Darwin arm64') platform=macos-arm64 ;;
    'Linux x86_64') platform=linux-amd64 ;;
    'Linux aarch64' | 'Linux arm64') platform=linux-arm64 ;;
    *) refuse "mailtriage has no build for $os $arch; see the guide's Install section to build from source" ;;
  esac
  if [ -z "$tray" ]; then
    case "$os" in
      Darwin) tray=1 ;;
      *) if [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then tray=1; else tray=0; fi ;;
    esac
  fi

  tmp=$(mktemp -d) || fail "cannot create a temporary directory"
  trap 'rm -rf "$tmp"' EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM

  version=${version:-$default_version}
  if [ -z "$version" ]; then
    latest=$(fetch -o /dev/null -w '%{url_effective}' "$base/releases/latest") ||
      fail "cannot find the newest release at $base/releases/latest"
    tag=${latest#"$base/releases/tag/v"}
    if [ "$tag" = "$latest" ] || ! is_version "$tag"; then
      fail "the newest release is at an unexpected URL: $latest"
    fi
    version=$tag
  fi

  download="$base/releases/download/v$version"
  cli_archive="mailtriage-v$version-$platform.tar.gz"
  tray_archive="mailtriage-tray-v$version-$platform.tar.gz"
  say "installing mailtriage $version ($platform) into $dir"
  fetch -o "$tmp/$cli_archive" "$download/$cli_archive" || fail "cannot download $cli_archive"
  fetch -o "$tmp/SHA256SUMS" "$download/SHA256SUMS" || fail "cannot download SHA256SUMS of v$version"
  if [ "$tray" = 1 ]; then
    fetch -o "$tmp/$tray_archive" "$download/$tray_archive" || fail "cannot download $tray_archive"
  fi
  verify "$cli_archive"
  unpack "$cli_archive" mailtriage LICENSE README.md
  if [ "$tray" = 1 ]; then
    verify "$tray_archive"
    unpack "$tray_archive" mailtriage-tray LICENSE
  fi

  set -- self install --dir "$dir"
  if [ "$tray" = 1 ]; then set -- "$@" --tray-file "$tmp/mailtriage-tray"; fi
  if [ "$no_setup" = 1 ]; then set -- "$@" --no-setup; fi
  if [ "$yes" = 1 ]; then set -- "$@" --yes; fi
  status=0
  "$tmp/mailtriage" "$@" <"$(input)" || status=$?
  # 126: the shell found the program but could not run it.
  if [ "$status" = 126 ]; then
    say "could not run the downloaded mailtriage from the temporary directory ${tmp%/*} (exit 126); it may be mounted noexec"
    say "run the installer again with a temporary directory that does: curl ... | TMPDIR=<a directory that allows running programs> sh"
  fi
  exit "$status"
}

main "$@"
}
