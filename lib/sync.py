import os
import sys
import requests
import sqlite3
from dotenv import load_dotenv

# Resolve paths relative to this script, not the caller's CWD.
# This ensures sync.py always finds the right .env and DB regardless
# of where the user invokes noguisteam from.
SCRIPT_DIR = os.path.dirname(os.path.realpath(__file__))
PROJECT_ROOT = os.path.abspath(os.path.join(SCRIPT_DIR, ".."))
DB_PATH = os.path.join(PROJECT_ROOT, "steam_games.db")

load_dotenv(os.path.join(PROJECT_ROOT, ".env"))

STEAM_API_KEY = os.getenv("STEAM_API_KEY")
STEAM_ID = os.getenv("STEAM_ID")

if not STEAM_API_KEY or not STEAM_ID:
    raise ValueError("STEAM_API_KEY or STEAM_ID not set in .env file")

URL = "https://api.steampowered.com/IPlayerService/GetOwnedGames/v1/"

params = {
    "key": STEAM_API_KEY,
    "steamid": STEAM_ID,
    "include_appinfo": True,
    "include_played_free_games": True,
}

response = requests.get(URL, params=params)
response.raise_for_status()

games = response.json()["response"].get("games", [])

if not games:
    print("No games returned from Steam API.")
    sys.exit(0)

steamapps_path = os.path.expanduser("~/.steam/steam/steamapps/")

conn = sqlite3.connect(DB_PATH)
c = conn.cursor()

c.execute("""
CREATE TABLE IF NOT EXISTS games (
    appid INTEGER PRIMARY KEY,
    name TEXT,
    playtime_forever INTEGER,
    last_played INTEGER,
    installed BOOLEAN
)
""")

# Upsert every owned game, re-checking installed status from disk each time
for g in games:
    appid = g["appid"]
    last_played = g.get("rtime_last_played", 0)
    manifest_path = os.path.join(steamapps_path, f"appmanifest_{appid}.acf")
    installed = 1 if os.path.exists(manifest_path) else 0

    c.execute("""
    INSERT OR REPLACE INTO games (appid, name, playtime_forever, last_played, installed)
    VALUES (?, ?, ?, ?, ?)
    """, (
        appid,
        g["name"],
        g.get("playtime_forever", 0),
        last_played,
        installed,
    ))

# Fix stale installed=1 rows for games uninstalled outside of noguisteam
# (e.g. via Steam client directly)
c.execute("SELECT appid FROM games WHERE installed=1")
stale = 0
for (appid,) in c.fetchall():
    manifest_path = os.path.join(steamapps_path, f"appmanifest_{appid}.acf")
    if not os.path.exists(manifest_path):
        c.execute("UPDATE games SET installed=0 WHERE appid=?", (appid,))
        stale += 1

conn.commit()
conn.close()

print(f"Synced {len(games)} games to {DB_PATH}")
if stale:
    print(f"Cleared stale 'installed' flag for {stale} game(s) (manifests missing on disk).")
