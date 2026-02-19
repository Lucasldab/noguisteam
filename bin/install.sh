#!/usr/bin/env bash
set -euo pipefail

echo "====================================="
echo "Welcome to noguisteam setup"
echo "====================================="

# 1. Check core dependencies
echo "Checking core dependencies..."
for dep in git steam sqlite3 python3; do
    if ! command -v $dep >/dev/null 2>&1; then
        echo "❌ $dep is not installed. Please install it first."
        exit 1
    fi
done
echo "✅ Core dependencies found."

# 2. Check fzf
if ! command -v fzf >/dev/null 2>&1; then
    echo "fzf not found. Installing..."
    # Linux: install from git if not found
    if command -v git >/dev/null 2>&1; then
        tmpdir=$(mktemp -d)
        git clone --depth 1 https://github.com/junegunn/fzf.git "$tmpdir/fzf"
        "$tmpdir/fzf/install" --bin
        rm -rf "$tmpdir"
        echo "✅ fzf installed."
    else
        echo "❌ git not found. Cannot install fzf automatically."
        exit 1
    fi
else
    echo "✅ fzf found."
fi

# 3. Create .env if missing
if [[ ! -f .env ]]; then
    echo "Creating example .env file..."
    cp .env.example .env
    echo "✅ .env created. Fill in your Steam API key, Steam ID, and username."
fi

# 4. Make scripts executable
chmod +x bin/noguisteam
echo "✅ Scripts made executable."

# 5. Install Python dependencies if needed
if [[ -f requirements.txt ]]; then
    echo "Installing Python dependencies..."
    python3 -m pip install --user -r requirements.txt
fi

echo "====================================="
echo "Setup complete! You can now run ./bin/noguisteam"
echo "====================================="

