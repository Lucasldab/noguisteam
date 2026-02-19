#!/usr/bin/env bash
# env.sh
# Loads project environment and defines core paths

set -euo pipefail

############################################
# Determine project root dynamically
############################################
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

############################################
# Core project paths
############################################
ENV_PATH="$PROJECT_ROOT/.env"
DB="$PROJECT_ROOT/steam_games.db"

############################################
# Steam paths (single source of truth)
############################################
STEAMLIB="$HOME/.steam/steam/steamapps"
INSTALL_DIR="$STEAMLIB/common"
STEAMCMD="${STEAMCMD:-/usr/bin/steamcmd}"

############################################
# Load .env file
############################################
load_env() {
    if [[ ! -f "$ENV_PATH" ]]; then
        echo "❌ .env file not found at $ENV_PATH"
        exit 1
    fi

    # shellcheck disable=SC2046
    export $(grep -v '^#' "$ENV_PATH" | xargs)

    validate_env
}

############################################
# Validate required environment variables
############################################
validate_env() {
    local required_vars=(
        STEAM_USERNAME
        STEAM_ID
    )

    for var in "${required_vars[@]}"; do
        if [[ -z "${!var:-}" ]]; then
            echo "❌ Required variable '$var' not defined in .env"
            exit 1
        fi
    done

    STEAM_USER="$STEAM_USERNAME"
}

############################################
# Initialize environment immediately
############################################
load_env

