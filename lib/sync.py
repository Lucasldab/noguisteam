import os
import requests
import sqlite3
from datetime import datetime
from dotenv import load_dotenv

load_dotenv()

STEAM_API_KEY = os.getenv("STEAM_API_KEY")
STEAM_ID = os.getenv("STEAM_ID")

if not STEAM_API_KEY or not STEAM_ID:
    raise ValueError("STEAM_API_KEY or STEAM_ID not set in .env file")

URL = "https://api.steampowered.com/IPlayerService/GetOwnedGames/v1/"

params = {
    "key": STEAM_API_KEY,
    "steamid": STEAM_ID,
    "include_appinfo": True,
    "include_played_free_games": True
}

response = requests.get(URL, params=params)
response.raise_for_status()

games = response.json()["response"].get("games", [])

steamapps_path = os.path.expanduser("~/.steam/steam/steamapps/")

conn = sqlite3.connect("steam_games.db")
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

for g in games:
    appid = g["appid"]
    last_played = g.get("rtime_last_played", 0)
    installed = 1 if os.path.exists(os.path.join(steamapps_path, f"appmanifest_{appid}.acf")) else 0

    c.execute("""
    INSERT OR REPLACE INTO games (appid, name, playtime_forever, last_played, installed)
    VALUES (?, ?, ?, ?, ?)
    """, (
        appid,
        g["name"],
        g.get("playtime_forever", 0),
        last_played,
        installed
    ))

conn.commit()
conn.close()
print(f"Updated {len(games)} games in steam_games.db")

