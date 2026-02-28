#!/usr/bin/env bash
# ui.sh

set -euo pipefail

# ─────────────────────────────────────────────
# icat availability check (requires Kitty terminal)
# ─────────────────────────────────────────────
ICAT_AVAILABLE=0
if [[ "${TERM:-}" == "xterm-kitty" ]] && kitty +kitten icat --version &>/dev/null 2>&1; then
    ICAT_AVAILABLE=1
fi

CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/noguisteam/headers"
mkdir -p "$CACHE_DIR"

# ─────────────────────────────────────────────
# Fetch and cache game header image
# ─────────────────────────────────────────────
fetch_header_image() {
    local appid="$1"
    local cache_file="$CACHE_DIR/${appid}.jpg"

    if [[ ! -f "$cache_file" ]]; then
        local url="https://cdn.cloudflare.steamstatic.com/steam/apps/${appid}/header.jpg"
        curl -sf --max-time 5 -o "$cache_file" "$url" || {
            rm -f "$cache_file"
            return 1
        }
    fi

    echo "$cache_file"
}

# ─────────────────────────────────────────────
# Render image via kitty icat
# ─────────────────────────────────────────────
render_image() {
    local image_path="$1"

    [[ ! -f "$image_path" ]] && return 1
    [[ "$ICAT_AVAILABLE" -eq 0 ]] && return 1

    kitty +kitten icat \
        --align left \
        --scale-up \
        --place 55x12@0x0 \
        "$image_path" 2>/dev/null
}

# ─────────────────────────────────────────────
# fzf preview pane: image + game info table
# ─────────────────────────────────────────────
preview_game() {
    local appid="$1"

    [[ "$appid" =~ ^[0-9]+$ ]] || {
        echo "  No game selected."
        exit 0
    }

    # --- Image ---
    if [[ "$ICAT_AVAILABLE" -eq 1 ]]; then
        local image_path
        if image_path=$(fetch_header_image "$appid" 2>/dev/null); then
            render_image "$image_path"
        fi
    fi

    # --- Info table ---
    echo ""
    printf "  %-16s %s\n" "AppID:" "$appid"

    sqlite3 "$DB" "
        SELECT
            name,
            printf('%dh %dm', playtime_forever/60, playtime_forever%60),
            CASE last_played
                WHEN 0 THEN 'Never'
                ELSE datetime(last_played, 'unixepoch')
            END,
            CASE installed
                WHEN 1 THEN '✅ Installed'
                ELSE '○  Not installed'
            END
        FROM games
        WHERE appid = $appid;
    " | while IFS='|' read -r name playtime last_played status; do
        printf "  %-16s %s\n" "Game:"        "$name"
        printf "  %-16s %s\n" "Playtime:"    "$playtime"
        printf "  %-16s %s\n" "Last played:" "$last_played"
        printf "  %-16s %s\n" "Status:"      "$status"
    done
}

export -f preview_game fetch_header_image render_image
export DB ICAT_AVAILABLE CACHE_DIR

# ─────────────────────────────────────────────
# fzf game selector
# ─────────────────────────────────────────────
select_game_ui() {
    local selection

    selection=$(
        list_games_for_selection \
        | fzf \
            --ansi \
            --expect=I,U,P,L,W \
            --prompt="  🎮 > " \
            --header=$'I: Install   U: Uninstall   P: Play   L: Update Library   W: Wishlist Sales\n' \
            --header-first \
            --delimiter="|" \
            --with-nth=1 \
            --preview "bash $(realpath "$0") preview {2}" \
            --preview-window=right:50%:wrap \
            --color='header:italic:dim,prompt:cyan,pointer:cyan,hl:yellow,hl+:yellow'
    )

    [[ -z "$selection" ]] && return 1

    parse_selection "$selection"
}

# ─────────────────────────────────────────────
# Parse fzf output → key|name|appid
# ─────────────────────────────────────────────
parse_selection() {
    local raw="$1"
    local key line name appid

    key=$(echo "$raw" | head -n1)
    line=$(echo "$raw" | tail -n1)

    name=$(echo "$line"  | awk -F'|' '{print $1}' | sed 's/[[:space:]]*$//')
    appid=$(echo "$line" | awk -F'|' '{print $2}')

    echo "$key|$name|$appid"
}

# ─────────────────────────────────────────────
# Entry point
# ─────────────────────────────────────────────
run_ui() {
    local result

    if ! result=$(select_game_ui); then
        echo "No game selected."
        return 1
    fi

    echo "$result"
}

# ─────────────────────────────────────────────
# Preview mode — called by fzf subprocess
# ─────────────────────────────────────────────
if [[ "${1:-}" == "preview" ]]; then
    preview_game "${2:-}"
    exit 0
fi
