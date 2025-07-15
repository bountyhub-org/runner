#!/bin/bash

set -e

REPO="https://github.com/bountyhub-org/runner"
REPO_API="https://api.github.com/repos/bountyhub-org/runner"
BINARY_DST="${HOME}/.local/bin"

usage() {
    echo "Usage: $0 [options]"
    echo "Options:"
    echo "  -h, --help      Show this help message"
    echo "  -d, --destination <path>  Specify the destination directory for the binary (default: ${BINARY_DST})"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        -h|--help)
            usage
            exit 0
            ;;
        -d|--destination)
            if [[ -n "$2" && ! "$2" =~ ^- ]]; then
                BINARY_DST="$2"
                mkdir -p "${BINARY_DST}" || {
                    echo "Error: Failed to create destination directory ${BINARY_DST}."
                    exit 1
                }
                shift 2
            else
                echo "Error: --destination requires a path argument."
                exit 1
            fi
            ;;
        *)
            echo "Unknown option: $1"
            usage
            exit 1
            ;;
    esac
done

fail() {
    echo "Error: $1"
    exit 1
}

prepare_directories() {
    echo "Preparing binary destination directory at ${BINARY_DST}..."
    if [[ ! -d "${BINARY_DST}" ]]; then
        if ! mkdir -p "${BINARY_DST}"; then
            fail "Failed to create directory ${BINARY_DST}."
        fi
    fi
    if [[ ! -w "${BINARY_DST}" ]]; then
        fail "No write permission for ${BINARY_DST}. Please check your permissions."
    fi
    echo "Binary destination directory is ready."
}

mk_temp_dir() {
    local _dir
    _dir=$(mktemp -d)
    if [[ ! -d "$_dir" ]]; then
        fail "Failed to create temporary directory."
    fi
    trap 'rm -rf "$_dir"' EXIT
    echo "$_dir"
}

get_arch() {
    local _os_type _cpu_type
    _os_type="$(uname -s)"
    if [[ "$_os_type" == "Linux" ]]; then
        if [[ "$(uname -o)" == "Android" ]]; then
            fail "Android is not supported."
        fi
    else
        fail "Unsupported OS: $_os_type"
    fi

    _cpu_type="$(uname -m)"
    if [[ "$_cpu_type" != "x86_64" ]]; then
        fail "Unsupported CPU architecture: $_cpu_type"
    fi

    echo "x86_64-unknown-linux-gnu"
}

get_latest_tag() {
    local _latest_tag
    if ! _latest_tag=$(curl -s "$REPO_API/releases/latest" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/'); then
        fail "Failed to get latest release information."
    fi

    if [ -z "$_latest_tag" ]; then
        fail "Could not determine the latest release tag."
    fi

    echo "$_latest_tag"
}

# Download the latest release from GitHub
download_target_to() {
    local _temp_dir=$1
    local _latest_tag=$2
    local _arch=$3

    local _file_name="runner-${_latest_tag}-${_arch}.tar.gz"
    echo "Latest release: $_latest_tag"
    local _url="${REPO}/releases/download/${_latest_tag}/${_file_name}"

    echo "Downloading from $_url..."
    if ! curl -L "${_url}" -o "${_temp_dir}/${_file_name}"; then
        fail "Failed to download the latest release."
    fi

    echo "Download complete."
}

extract_target_to() {
    local _temp_dir=$1
    local _latest_tag=$2
    local _arch=$3
    local _archive="${_temp_dir}/runner-${_latest_tag}-${_arch}.tar.gz"
    local _result_dir="${_temp_dir}/runner-${_latest_tag}-${_arch}"

    echo "Extracting the tarball ${_archive}..."
    if ! tar -xzf "${_archive}" -C "${_temp_dir}"; then
        fail "Failed to extract the tarball."
    fi

    chmod +x "${_result_dir}/runner"
    if [[ ! -x "${_result_dir}/runner" ]]; then
        fail "The runner binary is not executable."
    fi

    echo "Extraction complete. Moving files to ${BINARY_DST}..."
    if ! mv "${_result_dir}/runner" "${BINARY_DST}/"; then
        fail "Failed to move the runner binary to ${BINARY_DST}."
    fi
}

main() {
    echo "Starting installation process..."

    prepare_directories

    local _temp_dir
    _temp_dir=$(mk_temp_dir)

    local _latest_tag
    _latest_tag=$(get_latest_tag)

    local _arch
    _arch=$(get_arch)

    echo "Downloading the latest ${_latest_tag} release for architecture ${_arch}..."
    download_target_to "${_temp_dir}" "${_latest_tag}" "${_arch}"

    echo "Extracting the downloaded archive..."
    extract_target_to "${_temp_dir}" "${_latest_tag}" "${_arch}"

    echo "Installation complete. The runner binary is now available at ${BINARY_DST}/runner."
    echo
    echo "If you want to run the runner binary, make sure ${BINARY_DST} is in your PATH."
    echo
    echo "You can run it by executing 'runner' from your terminal."
}

main
