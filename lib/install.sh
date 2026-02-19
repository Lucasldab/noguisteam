#!/usr/bin/env bash
# install.sh
# Handles SteamCMD installation logic

set -euo pipefail

############################################
# Resolve the correct installation path
############################################
resolve_install_path() {
    local name="$1"
    local appid="$2"

    local safe_name
    safe_name=$(echo "$name" | sed 's/[^a-zA-Z0-9_ -]/_/g')

    local path="$INSTALL_DIR/$safe_name"
    local manifest="$STEAMLIB/appmanifest_${appid}.acf"

    # If Steam already has a manifest, respect its installdir
    if [[ -f "$manifest" ]]; then
        local folder_name
        folder_name=$(grep '"installdir"' "$manifest" | cut -d'"' -f4 || true)

        if [[ -n "${folder_name:-}" ]]; then
            path="$INSTALL_DIR/$folder_name"
        fi
    fi

    echo "$path"
}

############################################
# Run SteamCMD installation
############################################
run_steamcmd_install() {
    local appid="$1"
    local path="$2"

    echo "Running SteamCMD install..."
    echo "AppID: $appid"
    echo "Install dir: $path"

    "$STEAMCMD" \
        +login "$STEAM_USER" \
        +force_install_dir "$path" \
        +app_update "$appid" validate \
        +quit
}

############################################
# Validate installation result
############################################
validate_installation() {
    local appid="$1"
    local path="$2"

    if [[ ! -d "$path" ]]; then
        echo "❌ Installation directory does not exist."
        return 1
    fi

    if [[ -z "$(ls -A "$path" 2>/dev/null)" ]]; then
        echo "❌ Installation directory is empty."
        return 1
    fi

    echo "✅ Installation directory verified."
    return 0
}

############################################
# Main installation entrypoint
############################################
install_game() {
    local name="$1"
    local appid="$2"

    echo "======================================"
    echo "Installing: $name ($appid)"
    echo "======================================"

    local path
    path=$(resolve_install_path "$name" "$appid")

    mkdir -p "$path"

    run_steamcmd_install "$appid" "$path"

    if ! validate_installation "$appid" "$path"; then
        echo "❌ Installation failed validation."
        return 1
    fi

    # Ensure Steam client recognizes it
    if ! ensure_manifest "$appid" "$path"; then
        echo "⚠️ Manifest creation failed."
    fi

    # Update DB
    mark_installed "$appid"

    echo "🎉 $name successfully installed and recorded."
}

