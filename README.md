# No GUI Steam

A terminal UI for managing a Steam library via **SteamCMD**. Install, uninstall, launch, sync, and watch wishlist sales — all from the keyboard.

Built in Rust with [ratatui](https://github.com/ratatui-org/ratatui). Steam client must still be installed; this tool complements it.

---

## Features

* Library tab — install / uninstall / launch games, incremental search, cover art preview.
* Wishlist tab — fetch active sales via [IsThereAnyDeal](https://isthereanydeal.com), sort by deal quality / discount / price, open store page.
* Local SQLite cache of your Steam library.
* Async installs and wishlist fetches — UI stays responsive.
* Panic-safe terminal restore.

---

## Requirements

* Linux
* [SteamCMD](https://developer.valvesoftware.com/wiki/SteamCMD) on `PATH` (Arch: AUR `steamcmd`), or set `STEAMCMD`
* Steam client, logged in (used to launch games and to find your account)

---

## Install

```bash
cargo install --locked --git https://github.com/Lucasldab/noguisteam
```

---

## Setup

1. Run `noguisteam`. The first run creates `~/.config/noguisteam/env` (mode 0600) and exits.
2. Put your [Steam Web API key](https://steamcommunity.com/dev/apikey) in it as `STEAM_API_KEY`.
   Optionally add an [IsThereAnyDeal key](https://isthereanydeal.com/apps/my/) (`ITAD_KEY`) for wishlist sales.
3. Run `noguisteam login` once. SteamCMD asks for your password and Steam Guard code
   and caches the session; installs and uninstalls use it from then on.
   Your password is never stored or put on a command line.
   SteamCMD runs from its own folder (`~/.local/share/noguisteam/steamcmd`) so the
   Steam client cannot log it out; games still install into the client's library.

Your Steam ID and login name are read from the local Steam client
(`~/.steam/steam/config/loginusers.vdf`). Set `STEAM_ID` / `STEAM_USERNAME` in the
env file to override them. Your Steam profile's game details must be **public** for library sync.

| Key | Purpose |
| --- | --- |
| `STEAM_API_KEY` | Steam Web API key (required) |
| `ITAD_KEY` | IsThereAnyDeal API key — wishlist sales |
| `COUNTRY` | Regional pricing code (e.g. `US`, `SE`, `BR`); default `US` |
| `AUTO_SYNC` | `1` to sync the library at every start |
| `STEAM_ID`, `STEAM_USERNAME` | Override the account found in the Steam client |
| `STEAMCMD` | Path to `steamcmd` if it is not on `PATH` |

Environment variables override the file. Data lives in `~/.local/share/noguisteam/`,
cover art in `~/.cache/noguisteam/`. Colors: copy `colors.example.json` to
`~/.config/noguisteam/colors.json`.

A checkout that still has a `.env` or `steam_games.db` in its root (the pre-0.2 layout)
keeps using those files.

---

## Usage

```bash
noguisteam          # open the TUI
noguisteam login    # cache the steamcmd login (once, or when it expires)
```

### Global

* `Tab` / `Shift+Tab` — switch tabs (Library / Wishlist / Stats)
* `q` — quit

### Library

* `↑` / `↓` or `k` / `j` — navigate
* `/` — incremental search (Esc to clear)
* `i` — install selected game
* `u` — uninstall selected game
* `p` — launch via Steam
* `l` — sync library from Steam Web API

### Wishlist

* `r` — fetch / refresh sales
* `s` — cycle sort (Best Deal → Discount → Price)
* `o` — open store page in browser
* `↑` / `↓` or `k` / `j` — navigate

---

## Source layout

* `src/main.rs` — entry point, event loop, key dispatch
* `src/setup.rs` — file locations, first-run settings, `login` command
* `src/app.rs` — app state (tabs, selection, install state, wishlist)
* `src/ui.rs` — ratatui rendering
* `src/steam.rs` — SteamCMD + Steam Web API + ITAD integration
* `src/steamcmd.rs` — SteamCMD's own Steam folder, sharing only the game files with the client
* `src/db.rs` — SQLite cache
* `src/cover.rs` — cover art download/cache + in-layout rendering via `ratatui-image`

---

## Contributing

1. Fork the repo
2. Branch: `git checkout -b feature/my-feature`
3. Commit and push
4. Open a PR

---

## License

MIT — see [LICENSE](LICENSE).

---

## Disclaimer

Not affiliated with, endorsed by, or associated with Valve Corporation or Steam. All trademarks belong to their respective owners.
