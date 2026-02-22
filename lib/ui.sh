#!/usr/bin/env bash
# ui.sh

set -euo pipefail

preview_game() {
    local appid="$1"

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

select_game_ui() {

    local selection

    selection=$(
        list_games_for_selection \
        | fzf \
            --ansi \
            --expect=I,U,P,L,W \
            --prompt="Select a game > " \
            --header="I=Install | U=Uninstall | P=Play | L=Update Library | W=Wishlisted Sales" \
            --preview "bash $(realpath "$0") preview \$(echo {} | awk -F'|' '{print \$2}')"\
            --preview-window=up:7:wrap
    )

    if [[ -z "$selection" ]]; then
        return 1
    fi

    parse_selection "$selection"
}

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

run_ui() {
    local result

    if ! result=$(select_game_ui); then
        echo "No game selected."
        return 1
    fi

    echo "$result"
}

if [[ "${1:-}" == "preview" ]]; then
    preview_game "$2"
    exit 0
fi

