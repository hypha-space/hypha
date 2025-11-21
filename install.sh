#!/bin/sh
# shellcheck shell=dash
# shellcheck disable=SC2039  # local is non-POSIX
#
# Licensed under the MIT license
# <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
# option. This file may not be copied, modified, or distributed
# except according to those terms.
#
# Hypha installer script based on the install script from the uv project
# by Astral (https://astral.sh/uv).

has_local() {
    # shellcheck disable=SC2034
    local _has_local
}

has_local 2>/dev/null || alias local=typeset

set -u

APP_NAME="hypha"
VERSION=""
BINARIES="hypha-gateway hypha-worker hypha-data hypha-scheduler hypha-certutil"

if [ -n "${HYPHA_INSTALLER_BASE_URL:-}" ]; then
    INSTALLER_BASE_URL="$HYPHA_INSTALLER_BASE_URL"
else
    INSTALLER_BASE_URL="${HYPHA_INSTALLER_GITHUB_BASE_URL:-https://github.com}"
fi
REPO_OWNER="${HYPHA_INSTALLER_OWNER:-hypha-space}"
REPO_NAME="${HYPHA_INSTALLER_REPO:-hypha}"

if [ -n "${HYPHA_PRINT_VERBOSE:-}" ]; then
    PRINT_VERBOSE="$HYPHA_PRINT_VERBOSE"
else
    PRINT_VERBOSE=${INSTALLER_PRINT_VERBOSE:-0}
fi
if [ -n "${HYPHA_PRINT_QUIET:-}" ]; then
    PRINT_QUIET="$HYPHA_PRINT_QUIET"
else
    PRINT_QUIET=${INSTALLER_PRINT_QUIET:-0}
fi
if [ -n "${HYPHA_NO_MODIFY_PATH:-}" ]; then
    NO_MODIFY_PATH="$HYPHA_NO_MODIFY_PATH"
else
    NO_MODIFY_PATH=${INSTALLER_NO_MODIFY_PATH:-0}
fi
FORCE_INSTALL_DIR="${HYPHA_INSTALL_DIR:-${CARGO_DIST_FORCE_INSTALL_DIR:-}}"
AUTH_TOKEN="${HYPHA_GITHUB_TOKEN:-}"

usage() {
    cat <<EOF
$APP_NAME installer

USAGE:
    install.sh [OPTIONS]

OPTIONS:
    -v, --verbose          Enable verbose output
    -q, --quiet            Disable progress output
        --no-modify-path   Don't update shell profiles
    -h, --help             Print this help
EOF
}

say() {
    if [ "0" = "$PRINT_QUIET" ]; then
        echo "$1"
    fi
}

say_verbose() {
    if [ "1" = "$PRINT_VERBOSE" ]; then
        echo "$1"
    fi
}

warn() {
    if [ "0" = "$PRINT_QUIET" ]; then
        local red
        local reset
        red=$(tput setaf 1 2>/dev/null || echo '')
        reset=$(tput sgr0 2>/dev/null || echo '')
        say "${red}WARN${reset}: $1" >&2
    fi
}

err() {
    if [ "0" = "$PRINT_QUIET" ]; then
        local red
        local reset
        red=$(tput setaf 1 2>/dev/null || echo '')
        reset=$(tput sgr0 2>/dev/null || echo '')
        say "${red}ERROR${reset}: $1" >&2
    fi
    exit 1
}

need_cmd() {
    if ! check_cmd "$1"; then
        err "need '$1' (command not found)"
    fi
}

check_cmd() {
    command -v "$1" > /dev/null 2>&1
}

ensure() {
    if ! "$@"; then
        err "command failed: $*"
    fi
}

ignore() {
    "$@"
}

downloader() {
    local _dld
    local _snap_curl=0
    if command -v curl > /dev/null 2>&1; then
        local _curl_path
        _curl_path=$(command -v curl)
        if echo "$_curl_path" | grep "/snap/" > /dev/null 2>&1; then
            _snap_curl=1
        fi
    fi

    if check_cmd curl && [ "$_snap_curl" = "0" ]; then
        _dld=curl
    elif check_cmd wget; then
        _dld=wget
    elif [ "$_snap_curl" = "1" ]; then
        say "snap-provided curl lacks permissions required for $APP_NAME installer"
        say "please reinstall curl via your package manager (e.g., apt)"
        exit 1
    else
        _dld='curl or wget'
    fi

    if [ "$1" = --check ]; then
        need_cmd "$_dld"
    elif [ "$_dld" = curl ]; then
        if [ -n "${AUTH_TOKEN:-}" ]; then
            curl -sSfL --header "Authorization: Bearer ${AUTH_TOKEN}" "$1" -o "$2"
        else
            curl -sSfL "$1" -o "$2"
        fi
    elif [ "$_dld" = wget ]; then
        if [ -n "${AUTH_TOKEN:-}" ]; then
            wget --header "Authorization: Bearer ${AUTH_TOKEN}" "$1" -O "$2"
        else
            wget "$1" -O "$2"
        fi
    else
        err "unknown downloader"
    fi
}

verify_checksum() {
    local _file="$1"
    local _checksum_style="$2"
    local _checksum_value="$3"
    local _calculated_checksum

    if [ -z "$_checksum_value" ]; then
        return 0
    fi
    case "$_checksum_style" in
        sha256)
            if ! check_cmd sha256sum; then
                warn "skipping sha256 verification; install sha256sum to enable"
                return 0
            fi
            _calculated_checksum="$(sha256sum -b "$_file" | awk '{printf $1}')"
            ;;
        *)
            warn "unknown checksum style: $_checksum_style"
            return 0
            ;;
    esac

    if [ "$_calculated_checksum" != "$_checksum_value" ]; then
        err "checksum mismatch for $_file
    want: $_checksum_value
    got:  $_calculated_checksum"
    fi
}

get_home() {
    if [ -n "${HOME:-}" ]; then
        echo "$HOME"
    elif [ -n "${USER:-}" ]; then
        getent passwd "$USER" | cut -d: -f6
    else
        getent passwd "$(id -un)" | cut -d: -f6
    fi
}

get_home_expression() {
    if [ -n "${HOME:-}" ]; then
        echo '$HOME'
    elif [ -n "${USER:-}" ]; then
        getent passwd "$USER" | cut -d: -f6
    else
        getent passwd "$(id -un)" | cut -d: -f6
    fi
}

replace_home() {
    local _str="$1"

    if [ -n "${HOME:-}" ]; then
        echo "$_str" | sed "s,$HOME,\$HOME,"
    else
        echo "$_str"
    fi
}

print_home_for_script() {
    local script="$1"
    local _home

    case "$script" in
        .zsh*)
            if [ -n "${ZDOTDIR:-}" ]; then
                _home="$ZDOTDIR"
            else
                _home="$INFERRED_HOME"
            fi
            ;;
        *)
            _home="$INFERRED_HOME"
            ;;
    esac

    echo "$_home"
}

add_install_dir_to_ci_path() {
    local _install_dir="$1"

    if [ -n "${GITHUB_PATH:-}" ]; then
        ensure echo "$_install_dir" >> "$GITHUB_PATH"
    fi
}

write_env_script_sh() {
    local _install_dir_expr="$1"
    local _env_script_path="$2"
    ensure cat <<EOF > "$_env_script_path"
#!/bin/sh
case ":\${PATH}:" in
    *:"$_install_dir_expr":*)
        ;;
    *)
        export PATH="$_install_dir_expr:\$PATH"
        ;;
esac
EOF
}

write_env_script_fish() {
    local _install_dir_expr="$1"
    local _env_script_path="$2"
    ensure cat <<EOF > "$_env_script_path"
if not contains "$_install_dir_expr" \$PATH
    set -x PATH "$_install_dir_expr" \$PATH
end
EOF
}

add_install_dir_to_path() {
    local _install_dir_expr="$1"
    local _env_script_path="$2"
    local _env_script_path_expr="$3"
    local _rcfiles="$4"
    local _shell="$5"

    if [ -z "${INFERRED_HOME:-}" ]; then
        return 0
    fi

    local _target=""
    local _home

    for _rcfile_relative in $_rcfiles; do
        _home="$(print_home_for_script "$_rcfile_relative")"
        local _rcfile="$_home/$_rcfile_relative"
        if [ -f "$_rcfile" ]; then
            _target="$_rcfile"
            break
        fi
    done

    if [ -z "$_target" ]; then
        local _rcfile_relative
        _rcfile_relative="$(echo "$_rcfiles" | awk '{ print $1 }')"
        _home="$(print_home_for_script "$_rcfile_relative")"
        _target="$_home/$_rcfile_relative"
    fi

    local _robust_line=". \"$_env_script_path_expr\""
    local _pretty_line="source \"$_env_script_path_expr\""

    if [ ! -f "$_env_script_path" ]; then
        say_verbose "creating $_env_script_path"
        if [ "$_shell" = "sh" ]; then
            write_env_script_sh "$_install_dir_expr" "$_env_script_path"
        else
            write_env_script_fish "$_install_dir_expr" "$_env_script_path"
        fi
    fi

    if ! grep -F "$_robust_line" "$_target" > /dev/null 2>/dev/null && \
       ! grep -F "$_pretty_line" "$_target" > /dev/null 2>/dev/null; then
        if [ -f "$_env_script_path" ]; then
            local _line
            if [ "$_shell" = "fish" ]; then
                _line="$_pretty_line"
            else
                _line="$_robust_line"
            fi
            say_verbose "adding PATH source line to $_target"
            ensure echo "" >> "$_target"
            ensure echo "$_line" >> "$_target"
            return 1
        fi
    else
        say_verbose "$_install_dir_expr already configured in $_target"
    fi

    return 0
}

shotgun_install_dir_to_path() {
    local _install_dir_expr="$1"
    local _env_script_path="$2"
    local _env_script_path_expr="$3"
    local _rcfiles="$4"
    local _shell="$5"

    if [ -z "${INFERRED_HOME:-}" ]; then
        return 0
    fi

    local _found=false

    for _rcfile_relative in $_rcfiles; do
        local _home
        _home="$(print_home_for_script "$_rcfile_relative")"
        local _rcfile_abs="$_home/$_rcfile_relative"
        if [ -f "$_rcfile_abs" ]; then
            _found=true
            add_install_dir_to_path "$_install_dir_expr" "$_env_script_path" "$_env_script_path_expr" "$_rcfile_relative" "$_shell"
        fi
    done

    if [ "$_found" = false ]; then
        add_install_dir_to_path "$_install_dir_expr" "$_env_script_path" "$_env_script_path_expr" "$_rcfiles" "$_shell"
    fi
}

check_for_shadowed_bins() {
    local _install_dir="$1"
    local _bins="$2"
    local _shadowed=""

    for _bin_name in $_bins; do
        local _existing
        _existing="$(command -v "$_bin_name" 2>/dev/null || true)"
        if [ -n "$_existing" ] && [ "$_existing" != "$_install_dir/$_bin_name" ]; then
            _shadowed="$_shadowed $_bin_name"
        fi
    done

    echo "$_shadowed"
}

get_architecture() {
    local _ostype
    local _cputype

    _ostype="$(uname -s)"
    _cputype="$(uname -m)"

    case "$_ostype" in
        Linux)
            _ostype="unknown-linux"
            ;;
        Darwin)
            _ostype="apple-darwin"
            ;;
        *)
            err "unsupported operating system: $_ostype"
            ;;
    esac

    case "$_cputype" in
        x86_64 | x86-64 | x64 | amd64)
            _cputype="x86_64"
            ;;
        arm64 | aarch64)
            _cputype="aarch64"
            ;;
        *)
            err "unsupported CPU type: $_cputype"
            ;;
    esac

    RETVAL="${_cputype}-${_ostype}"
}

select_target_for_arch() {
    case "$1" in
        x86_64-unknown-linux)
            echo "x86_64-unknown-linux-musl"
            ;;
        aarch64-apple-darwin)
            echo "aarch64-apple-darwin"
            ;;
        *)
            err "no prebuilt binaries available for $1"
            ;;
    esac
}

download_artifact() {
    local _artifact="$1"
    local _dest="$2"
    local _url="$ARTIFACT_DOWNLOAD_URL/$_artifact"
    local _checksum_url="${_url}.sha256"
    local _checksum_tmp="${_dest}.sha256"

    say "downloading $_artifact"
    say_verbose "  from $_url"
    if ! downloader "$_url" "$_dest"; then
        err "failed to download $_url"
    fi

    if downloader "$_checksum_url" "$_checksum_tmp"; then
        local _checksum_value
        _checksum_value="$(awk '{print $1}' "$_checksum_tmp" | tr -d '\r\n')"
        if [ -n "$_checksum_value" ]; then
            verify_checksum "$_dest" sha256 "$_checksum_value"
        else
            warn "checksum file for $_artifact is empty; skipping verification"
        fi
        ignore rm -f "$_checksum_tmp"
    else
        warn "unable to download checksum for $_artifact; skipping verification"
    fi
}

install() {
    local _src_dir="$1"
    local _bins="$2"
    local _arch="$3"

    local _install_dir=""
    local _env_script_path=""
    local _install_dir_expr=""
    local _env_script_path_expr=""

    if [ -n "${FORCE_INSTALL_DIR:-}" ]; then
        _install_dir="$FORCE_INSTALL_DIR"
        _env_script_path="$FORCE_INSTALL_DIR/env"
        _install_dir_expr="$(replace_home "$_install_dir")"
        _env_script_path_expr="$(replace_home "$_env_script_path")"
    elif [ -n "${XDG_BIN_HOME:-}" ]; then
        _install_dir="$XDG_BIN_HOME"
        _env_script_path="$XDG_BIN_HOME/env"
        _install_dir_expr="$(replace_home "$_install_dir")"
        _env_script_path_expr="$(replace_home "$_env_script_path")"
    elif [ -n "${XDG_DATA_HOME:-}" ]; then
        _install_dir="$XDG_DATA_HOME/../bin"
        _env_script_path="$XDG_DATA_HOME/../bin/env"
        _install_dir_expr="$(replace_home "$_install_dir")"
        _env_script_path_expr="$(replace_home "$_env_script_path")"
    elif [ -n "${INFERRED_HOME:-}" ]; then
        _install_dir="$INFERRED_HOME/.local/bin"
        _env_script_path="$INFERRED_HOME/.local/bin/env"
        _install_dir_expr="$INFERRED_HOME_EXPRESSION/.local/bin"
        _env_script_path_expr="$INFERRED_HOME_EXPRESSION/.local/bin/env"
    fi

    if [ -z "$_install_dir" ] || [ -z "$_install_dir_expr" ]; then
        err "could not determine installation directory"
    fi

    local _fish_env_script_path="${_env_script_path}.fish"
    local _fish_env_script_path_expr="${_env_script_path_expr}.fish"

    say "installing to $_install_dir"
    ensure mkdir -p "$_install_dir"

    for _bin_name in $_bins; do
        local _src="$_src_dir/$_bin_name"
        ensure mv "$_src" "$_install_dir/"
        ensure chmod +x "$_install_dir/$_bin_name"
        say "  $_bin_name"
    done
    say "everything's installed!"

    case :$PATH: in
        *:$_install_dir:*)
            NO_MODIFY_PATH=1
            ;;
        *)
            ;;
    esac

    if [ "$NO_MODIFY_PATH" != "1" ]; then
        add_install_dir_to_ci_path "$_install_dir"
        add_install_dir_to_path "$_install_dir_expr" "$_env_script_path" "$_env_script_path_expr" ".profile" "sh"
        shotgun_install_dir_to_path "$_install_dir_expr" "$_env_script_path" "$_env_script_path_expr" ".profile .bashrc .bash_profile .bash_login" "sh"
        add_install_dir_to_path "$_install_dir_expr" "$_env_script_path" "$_env_script_path_expr" ".zshrc .zshenv" "sh"
        if [ -n "${INFERRED_HOME:-}" ]; then
            ensure mkdir -p "$INFERRED_HOME/.config/fish/conf.d"
        fi
        add_install_dir_to_path "$_install_dir_expr" "$_fish_env_script_path" "$_fish_env_script_path_expr" ".config/fish/conf.d/$APP_NAME.env.fish" "fish"
    fi

    local _shadowed
    _shadowed="$(check_for_shadowed_bins "$_install_dir" "$_bins")"
    if [ -n "$_shadowed" ]; then
        warn "commands shadowed by existing binaries in PATH:$_shadowed"
    fi
}

download_binary_and_run_installer() {
    downloader --check
    need_cmd uname
    need_cmd mktemp
    need_cmd chmod
    need_cmd mkdir
    need_cmd rm
    need_cmd awk
    need_cmd cut
    need_cmd tr
    need_cmd grep
    need_cmd sed

    while [ $# -gt 0 ]; do
        case "$1" in
            --help|-h)
                usage
                exit 0
                ;;
            --quiet|-q)
                PRINT_QUIET=1
                ;;
            --verbose|-v)
                PRINT_VERBOSE=1
                ;;
            --no-modify-path)
                NO_MODIFY_PATH=1
                ;;
            *)
                err "unknown option $1"
                ;;
        esac
        shift
    done

    if [ -z "$VERSION" ]; then
        err "installer version missing; use the script bundled with an official release"
    fi

    if [ -n "${HYPHA_DOWNLOAD_URL:-}" ]; then
        ARTIFACT_DOWNLOAD_URL="$HYPHA_DOWNLOAD_URL"
    else
        ARTIFACT_DOWNLOAD_URL="${INSTALLER_BASE_URL}/${REPO_OWNER}/${REPO_NAME}/releases/download/${VERSION}"
    fi

    get_architecture
    local _arch="$RETVAL"
    local _target
    _target="$(select_target_for_arch "$_arch")"

    local _dir
    _dir="$(ensure mktemp -d)"

    for _bin in $BINARIES; do
        local _artifact="$_bin-$_target"
        local _dest="$_dir/$_bin"
        download_artifact "$_artifact" "$_dest"
    done

    install "$_dir" "$BINARIES" "$_target"
    local _retval=$?
    ignore rm -rf "$_dir"
    return "$_retval"
}

INFERRED_HOME=$(get_home)
INFERRED_HOME_EXPRESSION=$(get_home_expression)

download_binary_and_run_installer "$@" || exit 1
