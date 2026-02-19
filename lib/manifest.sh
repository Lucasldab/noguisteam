#!/usr/bin/env bash
# manifest.sh
# Handles Steam appmanifest generation and Steam client sync

set -euo pipefail

############################################
# Create an appmanifest file
############################################
create_manifest() {
    local appid="$1"
    local installdir="$2"

    local acf="$STEAMLIB/appmanifest_${appid}.acf"
    local folder_name
    folder_name="$(basename "$installdir")"

    echo "Creating appmanifest for AppID $appid..."

    if [[ ! -d "$installdir" ]]; then
        echo "❌ Install directory does not exist: $installdir"
        return 1
    fi

    local timestamp
    timestamp=$(date +%s)

    local size_on_disk
    size_on_disk=$(du -sb "$installdir" | awk '{print $1}')

    cat > "$acf" <<EOF
"AppState"
{
    "appid"        "$appid"
    "Universe"     "1"
    "name"         "$folder_name"
    "StateFlags"   "4"
    "installdir"   "$folder_name"
    "LastUpdated"  "$timestamp"
    "UpdateResult" "0"
    "SizeOnDisk"   "$size_on_disk"
    "buildid"      "0"
    "LastOwner"    "$STEAM_ID"
    "BytesToDownload" "0"
    "BytesDownloaded" "0"
    "AutoUpdateBehavior" "2"
    "AllowOtherDownloadsWhileRunning" "1"
}
EOF

    echo "✅ Manifest written to: $acf"
}

############################################
# Check if manifest exists
############################################
manifest_exists() {
    local appid="$1"
    local acf="$STEAMLIB/appmanifest_${appid}.acf"

    [[ -f "$acf" ]]
}

############################################
# Gracefully restart Steam client
############################################
restart_steam() {
    echo "Refreshing Steam client..."

    if pgrep -x steam >/dev/null 2>&1; then
        echo "Closing running Steam instance..."
        pkill -x steam
        sleep 2
    fi

    # Relaunch silently
    steam -silent &
    disown

    echo "Steam restarted."
}

############################################
# Ensure Steam recognizes the installation
############################################
ensure_manifest() {
    local appid="$1"
    local installdir="$2"

    local acf="$STEAMLIB/appmanifest_${appid}.acf"

    if manifest_exists "$appid"; then
        echo "Manifest already exists for $appid."
        return 0
    fi

    create_manifest "$appid" "$installdir"

    if manifest_exists "$appid"; then
        restart_steam
        return 0
    else
        echo "❌ Failed to create manifest."
        return 1
    fi
}

