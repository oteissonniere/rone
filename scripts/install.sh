#!/bin/sh
# rone end-user installer: download the latest release from GitHub, verify its
# SHA-256 checksum and install it into ~/.local/share/rone/<version>/ with a
# symlink in ~/.local/bin. Bundled presets come along next to the binary. Only
# the installed version is kept: previously installed versions are removed.
#
#   curl -fsSL https://raw.githubusercontent.com/oteissonniere/rone/main/scripts/install.sh | sh
#
# The repository must be public for this one-liner to work (the script reads
# it from raw.githubusercontent.com and downloads release assets unauthenticated).
#
# Usage:
#   install.sh [VERSION]     install a specific version, e.g. 0.1.0
#                            (default: the latest GitHub Release)
#   install.sh --dry-run     print what would be done without downloading
#                            (resolves the latest release online when reachable)
#
# Environment overrides (for testing):
#   RONE_REPO      owner/repo to fetch from (default: oteissonniere/rone)

set -eu

REPO="${RONE_REPO:-oteissonniere/rone}"
BIN_NAME="rone"

version=""
dry_run=0
for arg in "$@"; do
    case "$arg" in
        --dry-run) dry_run=1 ;;
        -h|--help)
            sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) version="$arg" ;;
    esac
done

die() { printf '%s\n' "rone install: $1" >&2; exit 1; }

detect_target() {
    os=$(uname -s)
    arch=$(uname -m)
    case "$os" in
        Darwin)
            case "$arch" in
                arm64|aarch64) printf '%s' "aarch64-apple-darwin" ;;
                x86_64)        printf '%s' "x86_64-apple-darwin" ;;
                *)             printf '%s' unsupported ;;
            esac
            ;;
        Linux)
            case "$arch" in
                x86_64|amd64)  printf '%s' "x86_64-unknown-linux-gnu" ;;
                aarch64|arm64) printf '%s' "aarch64-unknown-linux-gnu" ;;
                *)             printf '%s' unsupported ;;
            esac
            ;;
        *) printf '%s' unsupported ;;
    esac
}

target=$(detect_target)
[ "$target" != "unsupported" ] || die "unsupported platform: $(uname -s) $(uname -m)" \
    "(supported: macOS and Linux, arm64/x86_64)"

resolve_latest_version() {
    json=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null) \
        || return 1
    tag=$(printf '%s\n' "$json" \
        | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
        | head -n 1)
    [ -n "$tag" ] || return 1
    printf '%s' "${tag#v}"
}

if [ -z "$version" ]; then
    version=$(resolve_latest_version) \
        && : \
        || {
            if [ "$dry_run" = 1 ]; then
                echo "note: latest release not resolved (offline?); pin a version for concrete URLs" >&2
                version="(latest, resolved at install time)"
            else
                die "could not reach the GitHub API for ${REPO} (is it public?)"
            fi
        }
fi

stem="rone-v${version}-${target}"
base_url="https://github.com/${REPO}/releases/download/v${version}"

echo "installing ${BIN_NAME} v${version} for ${target}"
if [ "$dry_run" = 1 ]; then
    echo "dry run: would download and verify"
    printf '  %s\n' "${base_url}/${stem}.tar.gz" "${base_url}/${stem}.tar.gz.sha256"
    echo "then extract into ~/.local/share/${BIN_NAME}/v${version}, link ~/.local/bin/${BIN_NAME}"
    echo "and remove any previously installed version directories"
    exit 0
fi

command -v curl >/dev/null || die "curl is required"
tmp=$(mktemp -d) || die "mktemp failed"
trap 'rm -rf "$tmp"' EXIT INT TERM

echo "downloading ${stem}.tar.gz"
curl -fsSL --output "${tmp}/${stem}.tar.gz" "${base_url}/${stem}.tar.gz" \
    || die "download of v${version} for ${target} failed (check the release assets)"
curl -fsSL --output "${tmp}/${stem}.tar.gz.sha256" "${base_url}/${stem}.tar.gz.sha256" \
    || die "could not download the checksum file"

cd "$tmp"
if command -v sha256sum >/dev/null; then
    echo "verifying checksum (sha256sum)"
    sha256sum -c "${stem}.tar.gz.sha256" || die "checksum mismatch - aborting install"
else
    echo "verifying checksum (shasum)"
    shasum -a 256 -c "${stem}.tar.gz.sha256" || die "checksum mismatch - aborting install"
fi

install_dir="${HOME}/.local/share/${BIN_NAME}/v${version}"
mkdir -p "$install_dir"
tar xzf "${tmp}/${stem}.tar.gz" -C "$install_dir" --strip-components=1 \
    || die "could not extract the tarball"
chmod +x "${install_dir}/${BIN_NAME}"

link_dir="${HOME}/.local/bin"
mkdir -p "$link_dir"
ln -sf "${install_dir}/${BIN_NAME}" "${link_dir}/${BIN_NAME}"

# Only the version just installed is kept: remove the previous ones.
for old in "${HOME}"/.local/share/"${BIN_NAME}"/v*; do
    [ -d "$old" ] || continue
    [ "$old" = "$install_dir" ] && continue
    printf 'removing previous version %s\n' "${old##*/}"
    rm -rf "$old"
done

case ":${PATH}:" in
    *":${link_dir}:"*) ;;
    *)
        echo ""
        echo "note: ${link_dir} is not on your PATH. Add this to your shell rc:"
        printf '  export PATH="$HOME/.local/bin:$PATH"\n'
        ;;
esac

echo ""
if "${install_dir}/${BIN_NAME}" --version; then
    echo "done: ${BIN_NAME} installed in ${install_dir}, linked as ${link_dir}/${BIN_NAME}"
else
    die "the installed binary did not run - check the output above"
fi
