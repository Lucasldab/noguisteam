#!/usr/bin/env bash
# install.sh

set -euo pipefail

resolve_install_path() {
    local name="$1"
    local appid="$2"

    local clean_name
    clean_name=$(echo "$name" | sed 's/ \[.*\]$//')

    local path="$INSTALL_DIR/$clean_name"
    local manifest="$STEAMLIB/appmanifest_${appid}.acf"
    if [[ -f "$manifest" ]]; then
        local folder_name
        folder_name=$(grep '"installdir"' "$manifest" | cut -d'"' -f4 || true)
        if [[ -n "$folder_name" ]]; then
            path="$INSTALL_DIR/$folder_name"
        fi
    fi

    echo "$path"
}

run_steamcmd_install() {
    local appid="$1"
    local path="$2"

    "$STEAMCMD" \
        +force_install_dir "$path" \
        +login "$STEAM_USER" \
        +app_update "$appid" validate \
        +quit
}

validate_installation() {
    local appid="$1"
    local path="$2"

    echo "Validating installation at: $path"

    if [[ ! -d "$path" ]]; then
        echo "❌ Installation directory does not exist."
        return 1
    fi

    if ! find "$path" -mindepth 1 -maxdepth 2 -type f -print -quit | grep -q .; then
        echo "❌ Installation directory is empty (no files found)."
        return 1
    fi

    echo "✅ Installation directory verified."
    return 0
}

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

    local manifest="$STEAMLIB/appmanifest_${appid}.acf"
    if [[ -f "$manifest" ]]; then
        local folder_name
        folder_name=$(grep '"installdir"' "$manifest" | cut -d'"' -f4 || true)
        if [[ -n "$folder_name" ]]; then
            path="$INSTALL_DIR/$folder_name"
        fi
    fi

    if ! validate_installation "$appid" "$path"; then
        echo "❌ Installation failed validation."
        return 1
    fi

    if ! ensure_manifest "$appid" "$path"; then
        echo "⚠️ Manifest creation failed."
    fi

    mark_installed "$appid"

    echo "🎉 $name successfully installed and recorded."
}

uninstall_game() {
    local name="$1"
    local appid="$2"

    echo "Uninstalling $name ($appid)..."

    local safe_name
    safe_name=$(echo "$name" | sed 's/[^a-zA-Z0-9_ -]/_/g')
    local path="$INSTALL_DIR/$safe_name"

    local manifest="$STEAMLIB/appmanifest_${appid}.acf"

    if [[ -f "$manifest" ]]; then
        local folder_name
        folder_name=$(grep '"installdir"' "$manifest" | cut -d'"' -f4 || true)
        if [[ -n "$folder_name" ]]; then
            path="$INSTALL_DIR/$folder_name"
        fi
    fi

    if [[ -d "$path" ]]; then
        rm -rf "$path"
        echo "Removed game files."
    fi

    if [[ -f "$manifest" ]]; then
        rm -f "$manifest"
        echo "Removed Steam manifest."
    fi

    sqlite3 "$DB" "UPDATE games SET installed=0 WHERE appid=$appid;"
    echo "$name fully uninstalled."
}

