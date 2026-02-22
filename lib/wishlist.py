"""
Steam Wishlist Sale Checker
============================
Fetches your Steam wishlist and checks which games are on sale,
highlighting Historical Lows, Matching Lowest, and 2-Year Lows
using the IsThereAnyDeal (ITAD) API.

Requirements:
    pip install requests python-dotenv rich

Setup:
    1. Get your Steam Web API key at: https://steamcommunity.com/dev/apikey
    2. Get your ITAD API key at: https://isthereanydeal.com/apps/my/
    3. Find your Steam ID (17-digit number) at: https://steamid.io
    4. Add to your .env file:
          STEAM_ID=your_17_digit_id
          STEAM_API_KEY=your_steam_api_key
          ITAD_KEY=your_itad_key
          COUNTRY=BR          ← must be set for BRL prices
"""

import os
import sys
import argparse
import requests
from dotenv import load_dotenv
from rich.console import Console
from rich.table import Table
from rich import box
from rich.text import Text

# ─────────────────────────────────────────────
# CONFIGURATION
# ─────────────────────────────────────────────
load_dotenv()

STEAM_ID      = os.getenv("STEAM_ID")
STEAM_API_KEY = os.getenv("STEAM_API_KEY")
ITAD_API_KEY  = os.getenv("ITAD_KEY")
COUNTRY       = os.getenv("COUNTRY", "US")

# Currency symbol map for display
CURRENCY_SYMBOLS = {
    "BR": "R$",
    "US": "$",
    "GB": "£",
    "DE": "€",
    "FR": "€",
    "AU": "A$",
}
CURRENCY_SYMBOL = CURRENCY_SYMBOLS.get(COUNTRY.upper(), "")

ITAD_STEAM_SHOP_ID = 61                      # Steam's numeric shop ID on ITAD
# ─────────────────────────────────────────────

STEAM_API_WISHLIST_URL   = "https://api.steampowered.com/IWishlistService/GetWishlist/v1"
STEAM_API_APPDETAILS_URL = "https://store.steampowered.com/api/appdetails"
ITAD_BASE_URL            = "https://api.isthereanydeal.com"

console = Console()


# ══════════════════════════════════════════════
# STEP 1 — Fetch Steam Wishlist
# ══════════════════════════════════════════════

def fetch_steam_wishlist(steam_id: str) -> dict[str, dict]:
    """
    Fetches wishlist via the official Steam Web API.
    Returns { app_id: { "name": str } }
    """
    console.print(f"[cyan]Fetching Steam wishlist for ID:[/cyan] {steam_id}")

    response = requests.get(
        STEAM_API_WISHLIST_URL,
        params={"key": STEAM_API_KEY, "steamid": steam_id},
        timeout=15,
    )

    if response.status_code != 200:
        console.print(f"[red]Steam API error {response.status_code}:[/red] {response.text[:300]}")
        sys.exit(1)

    items = response.json().get("response", {}).get("items", [])

    if not items:
        console.print("[yellow]Wishlist is empty or Steam returned no items.[/yellow]")
        return {}

    app_ids = [str(item["appid"]) for item in items if "appid" in item]
    console.print(f"[green]Found {len(app_ids)} games in wishlist.[/green]")

    # Resolve names via appdetails
    games = {}
    console.print("[cyan]Resolving game names...[/cyan]")

    for app_id in app_ids:
        try:
            r = requests.get(
                STEAM_API_APPDETAILS_URL,
                params={"appids": app_id, "filters": "basic"},
                timeout=10,
            )
            detail = r.json().get(app_id, {})
            name = detail["data"]["name"] if detail.get("success") else f"AppID {app_id}"
        except Exception:
            name = f"AppID {app_id}"

        games[app_id] = {"name": name}

    return games


# ══════════════════════════════════════════════
# STEP 2 — Look up ITAD game IDs from Steam IDs
# ══════════════════════════════════════════════

def itad_lookup_ids(app_ids: list[str]) -> dict[str, str]:
    """
    POST /lookup/id/shop/{shopId}/v1
    Body: ["app/12345", "app/67890", ...]
    Returns { steam_app_id: itad_uuid }
    """
    shop_ids = [f"app/{app_id}" for app_id in app_ids]

    response = requests.post(
        f"{ITAD_BASE_URL}/lookup/id/shop/{ITAD_STEAM_SHOP_ID}/v1",
        params={"key": ITAD_API_KEY},
        json=shop_ids,
        timeout=30,
    )

    if response.status_code != 200:
        console.print(f"[red]ITAD lookup error {response.status_code}:[/red] {response.text[:300]}")
        sys.exit(1)

    # Response: { "app/1234": "uuid-...", "app/5678": null, ... }
    data = response.json()
    id_map = {}
    for shop_id, itad_uuid in data.items():
        if itad_uuid:
            steam_app_id = shop_id.replace("app/", "")
            id_map[steam_app_id] = itad_uuid

    return id_map


# ══════════════════════════════════════════════
# STEP 3 — Get current prices & deal info
# ══════════════════════════════════════════════

def itad_get_prices(itad_ids: list[str], country: str) -> dict[str, dict]:
    """
    POST /games/prices/v3
    Returns { itad_uuid: price_data } for games currently on sale on Steam.

    The `country` parameter controls which regional pricing ITAD returns.
    For Brazil (BRL), set COUNTRY=BR in your .env.

    The response includes two useful low-price fields:
      - item["historyLow"]   → all-time low across ALL stores
      - deal["storeLow"]     → this store's (Steam's) all-time low
    """
    response = requests.post(
        f"{ITAD_BASE_URL}/games/prices/v3",
        params={
            "key":     ITAD_API_KEY,
            "country": country.upper(),
            "shops":   ITAD_STEAM_SHOP_ID,
            "deals":   1,
        },
        json=itad_ids,
        timeout=30,
    )

    if response.status_code != 200:
        console.print(f"[red]ITAD prices error {response.status_code}:[/red] {response.text[:300]}")
        sys.exit(1)

    prices = {}
    for item in response.json():
        game_id = item.get("id")
        deals   = item.get("deals", [])

        if not deals:
            continue

        # Best current deal on Steam
        best = min(deals, key=lambda d: d["price"]["amount"])

        # historyLow structure: { "all": {"amount": x}, "y1": {"amount": x}, "m3": {"amount": x} }
        history_low_obj = item.get("historyLow") or {}
        historical_low  = history_low_obj.get("all", {}).get("amount")  # all-time low
        one_year_low    = history_low_obj.get("y1",  {}).get("amount")  # 1-year low

        # Steam-specific all-time low (included in the deal object)
        steam_store_low_obj = best.get("storeLow")
        store_low           = steam_store_low_obj["amount"] if steam_store_low_obj else None

        prices[game_id] = {
            "current_price":    best["price"]["amount"],
            "regular_price":    best["regular"]["amount"],
            "discount_percent": best.get("cut", 0),
            "url":              best.get("url", ""),
            "store_low":        store_low,      # Steam's own all-time low
            "historical_low":   historical_low, # All-time low (any store)
            "one_year_low":     one_year_low,   # 1-year low (any store)
        }

    return prices


# ══════════════════════════════════════════════
# STEP 4 — Classify each deal
# ══════════════════════════════════════════════

DEAL_TAGS = {
    "store_low":       ("🔥 Steam All-Time Low", "bright_red"),
    "historical_low":  ("🏆 Cross-Store Low",    "dark_orange"),
    "matching_lowest": ("🔁 Matching Lowest",    "yellow"),
    "on_sale":         ("🏷️  On Sale",            "green"),
}

def classify_deal(price_data: dict) -> tuple[str, str]:
    current      = price_data["current_price"]
    store_low    = price_data.get("store_low")
    hist_low     = price_data.get("historical_low")
    one_year_low = price_data.get("one_year_low")

    if store_low is not None and current <= store_low:
        return DEAL_TAGS["store_low"]
    if hist_low is not None and current <= hist_low:
        return DEAL_TAGS["historical_low"]
    if one_year_low is not None and current <= one_year_low:
        return DEAL_TAGS["matching_lowest"]
    return DEAL_TAGS["on_sale"]


# ══════════════════════════════════════════════
# STEP 5 — Sort results
# ══════════════════════════════════════════════

TAG_ORDER = {
    "🔥 Steam All-Time Low": 0,
    "🏆 Cross-Store Low":    1,
    "🔁 Matching Lowest":    2,
    "🏷️  On Sale":            3,
}

def sort_results(results: list[dict], sort_by: str) -> list[dict]:
    """
    Sort results by one of three strategies:
      - "deal"      : best deal tag first, then by price (default)
      - "discount"  : highest discount % first
      - "price"     : lowest current price first
    """
    if sort_by == "discount":
        return sorted(results, key=lambda r: r["discount_percent"], reverse=True)
    elif sort_by == "price":
        return sorted(results, key=lambda r: r["current_price"])
    else:  # default: "deal"
        return sorted(results, key=lambda r: (TAG_ORDER.get(r["deal_tag"][0], 9), r["current_price"]))


# ══════════════════════════════════════════════
# STEP 6 — Display results
# ══════════════════════════════════════════════

def display_results(results: list[dict], sort_by: str) -> None:
    if not results:
        console.print("\n[yellow]No wishlist games are currently on sale on Steam.[/yellow]")
        return

    sort_label = {"deal": "Best Deal", "discount": "Discount %", "price": "Price"}.get(sort_by, sort_by)
    sym = CURRENCY_SYMBOL

    table = Table(
        title=f"🎮 Steam Wishlist — Current Sales  [dim](sorted by: {sort_label})[/dim]",
        box=box.ROUNDED,
        show_lines=True,
        header_style="bold magenta",
    )

    table.add_column("Game",          style="bold white", min_width=30, max_width=50)
    table.add_column("Sale",          justify="center",   min_width=22)
    table.add_column("Price",         justify="right",    style="green")
    table.add_column("Regular",       justify="right",    style="dim")
    table.add_column("Discount",      justify="center",   style="cyan")
    table.add_column("Steam Low",     justify="right",    style="dim")
    table.add_column("All-Store Low", justify="right",    style="dim")

    for r in results:
        label, color = r["deal_tag"]
        table.add_row(
            r["name"],
            Text(label, style=color),
            f"{sym}{r['current_price']:.2f}",
            f"{sym}{r['regular_price']:.2f}",
            f"-{r['discount_percent']}%",
            f"{sym}{r['store_low']:.2f}"      if r.get("store_low")      else "N/A",
            f"{sym}{r['historical_low']:.2f}" if r.get("historical_low") else "N/A",
        )

    console.print()
    console.print(table)
    console.print(f"\n[dim]{len(results)} games on sale | country: {COUNTRY.upper()} | sorted by: {sort_label}[/dim]")


# ══════════════════════════════════════════════
# MAIN
# ══════════════════════════════════════════════

def parse_args():
    parser = argparse.ArgumentParser(description="Steam Wishlist Sale Checker")
    parser.add_argument(
        "--sort", "-s",
        choices=["deal", "discount", "price"],
        default="deal",
        help=(
            "Sort order for results:\n"
            "  deal      — best deal tag first, then by price (default)\n"
            "  discount  — highest discount %% first\n"
            "  price     — lowest current price first"
        ),
    )
    return parser.parse_args()


def main():
    args = parse_args()

    if not STEAM_ID:
        console.print("[red]Error:[/red] STEAM_ID is not set in your .env file.")
        sys.exit(1)
    if not STEAM_API_KEY:
        console.print("[red]Error:[/red] STEAM_API_KEY is not set in your .env file.")
        console.print("Get one free at: https://steamcommunity.com/dev/apikey")
        sys.exit(1)
    if not ITAD_API_KEY:
        console.print("[red]Error:[/red] ITAD_KEY is not set in your .env file.")
        console.print("Get one free at: https://isthereanydeal.com/apps/my/")
        sys.exit(1)

    # 1. Fetch wishlist
    wishlist = fetch_steam_wishlist(STEAM_ID)
    if not wishlist:
        sys.exit(0)

    app_ids = list(wishlist.keys())

    # 2. Resolve Steam App IDs → ITAD UUIDs
    console.print("[cyan]Resolving ITAD game IDs...[/cyan]")
    steam_to_itad = itad_lookup_ids(app_ids)
    itad_to_steam = {v: k for k, v in steam_to_itad.items()}
    itad_ids = list(steam_to_itad.values())

    if not itad_ids:
        console.print("[red]No games resolved via ITAD. Check your API key.[/red]")
        sys.exit(1)

    console.print(f"[green]Resolved {len(itad_ids)} / {len(app_ids)} games.[/green]")

    # 3. Get current sale prices + historical lows (all in one request)
    console.print(f"[cyan]Fetching prices for country:[/cyan] {COUNTRY.upper()}")
    prices = itad_get_prices(itad_ids, COUNTRY)

    if not prices:
        console.print("[yellow]None of your wishlisted games are currently on sale.[/yellow]")
        sys.exit(0)

    # 4. Classify and assemble results
    results = []
    for itad_id, price_d in prices.items():
        steam_app_id = itad_to_steam.get(itad_id, "")
        name         = wishlist.get(steam_app_id, {}).get("name", f"ITAD:{itad_id}")

        results.append({
            "name":             name,
            "current_price":    price_d["current_price"],
            "regular_price":    price_d["regular_price"],
            "discount_percent": price_d["discount_percent"],
            "url":              price_d["url"],
            "store_low":        price_d.get("store_low"),
            "historical_low":   price_d.get("historical_low"),
            "deal_tag":         classify_deal(price_d),
        })

    # 5. Sort and display
    results = sort_results(results, args.sort)
    display_results(results, args.sort)


if __name__ == "__main__":
    main()
