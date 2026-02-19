# No GUI Steam

A lightweight terminal-based tool for managing your Steam games using **SteamCMD**.
Install, uninstall, play, and update your Steam library directly from the command line with a simple TUI (Text User Interface).

**Note:** Steam must be installed; this tool complements the Steam client and does not replace it.

---

## Features

* Install and uninstall Steam games without the Steam GUI.
* Play games directly from the terminal.
* Keep your Steam library in sync with a local database.
* Supports environment-based configuration for Steam credentials and API key.
* Simple, clean, and scriptable for automation.

---

## Requirements

* Linux or macOS
* [SteamCMD](https://developer.valvesoftware.com/wiki/SteamCMD)
* Git
* Python 3 (for library sync)
* Bash shell
* [fzf](https://github.com/junegunn/fzf) (for interactive TUI)

---
Absolutely — the README is the first place someone sees how to get your project running. If your project now has an installation script (especially since it handles dependencies like `fzf`), you should definitely **document it clearly**.

You could add a section like this in your README:

---

## Installation Script

This project includes an installation script that sets up required dependencies and prepares the environment.

```bash
# Make the script executable if not already
chmod +x bin/install.sh

# Run the installation
./bin/install.sh
```

**What it does:**

* Checks and installs required dependencies (like `fzf`)
* Sets up necessary directories and database
* Prepares the environment so the main script (`noguisteam`) can run

After running the installation script, you can run the main program:

```bash
./bin/noguisteam
```
---

## Manual Installation

Clone the repository:

```bash
git clone git@github.com:Lucasldab/noguisteam.git
cd noguisteam
```

Make the main script executable:

```bash
chmod +x bin/noguisteam
```

Create your environment file:

```bash
cp .env.example .env
```

Edit `.env` with your credentials:

```env
STEAM_API_KEY=your_steam_api_key
STEAM_ID=your_steam_id
STEAM_USERNAME=your_steam_username
```

> **Note:** Never commit your `.env` file with real credentials. Use `.env.example` as a template.

---

## Usage

Run the TUI:

```bash
./bin/noguisteam
```

Actions in the TUI:

* **I** – Install a game
* **U** – Uninstall a game
* **P** – Play a game
* **L** – Update library (sync with Steam)

---

## Development

The project is modular:

* `lib/env.sh` — loads environment variables
* `lib/db.sh` — handles the local database
* `lib/install.sh` — install/uninstall logic
* `lib/manifest.sh` — Steam manifest handling
* `lib/sync.py` — library synchronization
* `lib/ui.sh` — terminal user interface

---

## Contributing

1. Fork the repository.
2. Create a feature branch: `git checkout -b feature/my-feature`
3. Commit your changes: `git commit -m "Add my feature"`
4. Push to the branch: `git push origin feature/my-feature`
5. Open a Pull Request.

---

## License

This project is licensed under the MIT License – see the [LICENSE](LICENSE) file for details.

