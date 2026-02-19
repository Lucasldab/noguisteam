#!/usr/bin/env bash
# db.sh

set -euo pipefail

check_db() {
    if [[ ! -f "$DB" ]]; then
        echo "❌ Database not found at $DB"
        exit 1
    fi
}

mark_installed() {
    local appid="$1"

    sqlite3 "$DB" \
        "UPDATE games SET installed=1 WHERE appid=$appid;"
}

mark_uninstalled() {
    local appid="$1"

    sqlite3 "$DB" \
        "UPDATE games SET installed=0 WHERE appid=$appid;"
}

get_game_info() {
    local appid="$1"

    sqlite3 -column -header "$DB" "
        SELECT
            name,
            printf('%dh %dm', playtime_forever/60, playtime_forever%60) AS playtime,
            datetime(last_played, 'unixepoch') AS last_played,
            CASE installed
                WHEN 1 THEN 'Installed'
                ELSE 'Not installed'
            END AS status
        FROM games
        WHERE appid=$appid;
    "
}

list_games_for_selection() {
    sqlite3 "$DB" "
        SELECT name || ' [' ||
        CASE installed
            WHEN 1 THEN 'Installed'
            ELSE 'Not installed'
        END || ']' || '|' || appid
        FROM games
        ORDER BY installed ASC, name ASC;
    "
}

check_db

