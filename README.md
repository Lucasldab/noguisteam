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

* Linux or macOS
* [SteamCMD](https://developer.valvesoftware.com/wiki/SteamCMD) on `PATH` (or set `STEAMCMD`)
* Steam client (for `steam://run/<appid>` launching)
* Rust toolchain (stable) to build
* Optional: [`chafa`](https://hpjansson.org/chafa/) for cover art in the Library tab

---

## Build

```bash
git clone git@github.com:Lucasldab/noguisteam.git
cd noguisteam
cargo build --release
```

Binary: `target/release/noguisteam`.

---

## Configuration

Copy the template and fill in credentials:

```bash
cp .env.example .env
```

Keys:

| Key | Purpose |
| --- | --- |
| `STEAM_API_KEY` | Steam Web API key ([get one](https://steamcommunity.com/dev/apikey)) |
| `STEAM_ID` | Your 64-bit SteamID |
| `STEAM_USERNAME` | Steam login for SteamCMD |
| `STEAM_PASSWORD` | Steam password (optional; SteamCMD will prompt if unset) |
| `ITAD_KEY` | IsThereAnyDeal API key — required for wishlist sales ([get one](https://isthereanydeal.com/apps/my/)) |
| `COUNTRY` | Regional pricing code (e.g. `US`, `DE`, `GB`, `BR`) |
| `STEAMCMD` | Optional path to `steamcmd` binary |

The `.env` is loaded from the project root (resolved from the binary location or `NOGUISTEAM_HOME`). Your Steam profile must be **public** for library sync.

---

## Usage

```bash
./target/release/noguisteam
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
* `src/app.rs` — app state (tabs, selection, install state, wishlist)
* `src/ui.rs` — ratatui rendering
* `src/steam.rs` — SteamCMD + Steam Web API + ITAD integration
* `src/db.rs` — SQLite cache
* `src/image.rs` — cover art rendering via `chafa`

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
