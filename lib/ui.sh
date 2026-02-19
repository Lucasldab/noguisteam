#!/usr/bin/env bash
# ui.sh
# Handles all TUI / fzf interactions

set -euo pipefail

############################################
# Render preview for selected game
############################################
preview_game() {
    local appid="$1"

    # Ensure it's numeric (safety)
    [[ "$appid" =~ ^[0-9]+$ ]] || exit 0

    echo "| Name                           | Playtime   | Last Played          | Status"
    echo "--------------------------------------------------------------------------"

    sqlite3 "$DB" "
        SELECT printf(
            '| %-30s | %-10s | %-20s | %-12s',
            name,
            printf('%dh %dm', playtime_forever/60, playtime_forever%60),
            datetime(last_played, 'unixepoch'),
            CASE installed
                WHEN 1 THEN 'Installed'
                ELSE 'Not installed'
            END
        )
        FROM games
        WHERE appid = $appid;
    "
}


export -f preview_game
export DB

############################################
# Launch fzf selection UI
############################################
select_game_ui() {

    local selection

    selection=$(
        list_games_for_selection \
        | fzf \
            --ansi \
            --expect=I,U,L,P \
            --prompt="Select a game > " \
            --header="I=Install | U=Uninstall | L=Launch | P=Info" \
            --preview "bash $(realpath "$0") preview \$(echo {} | awk -F'|' '{print \$2}')"\
            --preview-window=up:7:wrap
    )

    if [[ -z "$selection" ]]; then
        return 1
    fi

    parse_selection "$selection"
}

############################################
# Parse fzf selection result
############################################
parse_selection() {
    local raw="$1"

    local key
    local line
    local name
    local appid

    key=$(echo "$raw" | head -n1)
    line=$(echo "$raw" | tail -n1)

    name=$(echo "$line" | awk -F'|' '{print $1}' | sed 's/[[:space:]]*$//')
    appid=$(echo "$line" | awk -F'|' '{print $2}')

    echo "$key|$name|$appid"
}

############################################
# High-level UI entrypoint
############################################
run_ui() {
    local result

    if ! result=$(select_game_ui); then
        echo "No game selected."
        return 1
    fi

    echo "$result"
}

############################################
# CLI dispatch (for fzf preview)
############################################
if [[ "${1:-}" == "preview" ]]; then
    preview_game "$2"
    exit 0
fi

