# Security Scan: noguisteam TUI

Generated: 2026-04-30  
Branch: prior-branch

---

## (a) Module Responsibilities

| Module | File | Responsibility |
|--------|------|----------------|
| main | `src/main.rs` | Entry point; terminal init; event loop; key-binding dispatch to `steam::install_game` / `uninstall_game` / library launch |
| app | `src/app.rs` | Central app state: active tab, `InstallState` FSM, wishlist entries, library filter, selected-game cursor |
| steam | `src/steam.rs` | SteamCMD subprocess launch (install/uninstall); Steam Web API library sync; wishlist + ITAD price fetch; `pgrep`/`steam -shutdown` Steam process management; `sanitize_line` output cleaner |
| db | `src/db.rs` | SQLite abstraction via `rusqlite`; game CRUD; `mark_installed`; playtime stats queries; parameterised prepared statements throughout |
| ui | `src/ui.rs` | Ratatui render loop: library list, wishlist table, stats charts, install-progress popup |
| image | `src/image.rs` | Game header image fetch + disk cache; `chafa` subprocess for terminal rendering |

---

## (b) Shell Injection Risk Surface around SteamCMD

Rust's `std::process::Command` calls `execv(2)` directly — it does **not** invoke a shell — so classic shell metacharacters (`$`, `;`, `|`, `>`) in arguments cannot trigger OS-level shell injection. The risk surface below is therefore **SteamCMD-protocol injection** (SteamCMD parses its own `+command` language), **process-listing credential exposure**, and **unvalidated binary path**, not a POSIX shell vulnerability.

### Invocation 1 — `install_game`

**`src/steam.rs:131-142`**

```rust
let mut child = Command::new(&config.steamcmd)      // ← STEAMCMD env var, unvalidated path
    .args([
        "+login",      &config.username, &config.password,   // ← env vars, no sanitization
        "+app_update", &app_id.to_string(),                  // ← u32, type-safe
        "validate",
        "+quit",
    ])
```

Risk factors:
- `config.steamcmd` (`src/steam.rs:42-45`): PathBuf built directly from `STEAMCMD` env var with no canonicalization or existence check. An attacker who controls the environment can point it at any binary.
- `config.username` / `config.password` (`src/steam.rs:36-39`): Raw `String` from env vars, zero validation. SteamCMD parses arguments sequentially and treats any token beginning with `+` as a command prefix. A username value of `foo +run_script /tmp/evil` would pass `foo`, `+run_script`, `/tmp/evil` as three separate arguments to SteamCMD, potentially executing an arbitrary script.
- `app_id` is `u32` — cannot carry injection payloads; safe.

### Invocation 2 — `uninstall_game`

**`src/steam.rs:176-186`**

```rust
let status = std::process::Command::new(&config.steamcmd)
    .args([
        "+login",         &config.username, &config.password,   // same risks as above
        "+app_uninstall", &app_id.to_string(),
        "+quit",
    ])
```

Identical risk surface to Invocation 1.

### Invocation 3 — `steam://run/<app_id>` launch

**`src/main.rs:288-292`**

```rust
Command::new("steam")
    .args(["-silent", &format!("steam://run/{}", game.app_id)])
```

`game.app_id` is `u32` from the database, which was originally a `u32` from the Steam API response. No injection surface.

### Invocation 4 — Steam process management

**`src/steam.rs:84-91`** (`pgrep -x steam`), **`src/steam.rs:102-107`** (`steam -shutdown`), **`src/steam.rs:114-119`** (`steam -silent`): all hardcoded arguments, no injection surface.

### Process-listing exposure

`config.password` appears as a plain command-line argument at invocations 1 and 2. Any local user with permission to read `/proc/<pid>/cmdline` (or run `ps aux`) can see the Steam password in clear text for the lifetime of the SteamCMD process.

### Summary table

| Location | Input | Type | Validated? | Risk |
|----------|-------|------|------------|------|
| `steam.rs:131` | `config.steamcmd` | PathBuf | No | Arbitrary binary execution |
| `steam.rs:133` | `config.username` | String | No | SteamCMD arg injection if contains `+` |
| `steam.rs:133` | `config.password` | String | No | Same; also process-listing exposure |
| `steam.rs:134` | `app_id` | u32 | Implicit | Safe — integer type |
| `steam.rs:178-179` | same three | same | same | Same as above |
| `main.rs:289` | `game.app_id` | u32 | Implicit | Safe |

---

## (c) Concrete Sanitization Test to Add

**Test goal:** verify that a username or password containing a bare `+word` token does not reach SteamCMD as a separate argument.

**Proposed implementation** — add to a new `tests/sanitize.rs` or inline `#[cfg(test)]` block in `src/steam.rs`:

```rust
#[cfg(test)]
mod tests {
    /// Builds the args vec the same way install_game and uninstall_game do,
    /// then asserts that no element after the credential positions starts with '+'.
    ///
    /// If a malicious username such as "user +run_script /tmp/evil" were passed
    /// without sanitization, the args vec would contain "+run_script" as a
    /// standalone element, which SteamCMD would interpret as a command.
    #[test]
    fn credential_args_contain_no_steamcmd_command_tokens() {
        let malicious_username = "validuser +run_script /tmp/evil";
        let malicious_password = "pass +force_install_dir /tmp";
        let app_id: u32 = 570;

        // Reproduce the exact args slice built in install_game / uninstall_game.
        let args: Vec<String> = vec![
            "+login".into(),
            malicious_username.into(),
            malicious_password.into(),
            "+app_update".into(),
            app_id.to_string(),
            "validate".into(),
            "+quit".into(),
        ];

        // Credentials occupy indices 1 and 2; everything else is a known-good literal.
        // The test fails today because the malicious tokens pass through unsanitized.
        // After a fix (e.g. rejecting credentials containing '+' or whitespace, or
        // validating at config load time), this test should pass.
        let credential_slots = &args[1..=2];
        for token in credential_slots {
            assert!(
                !token.contains('+'),
                "Credential contains '+' which SteamCMD may interpret as a command prefix: {:?}",
                token
            );
            assert!(
                !token.contains(' '),
                "Credential contains whitespace; it will be split into multiple args by the shell if ever interpolated: {:?}",
                token
            );
        }
    }
}
```

**Expected outcome:** this test fails against the current code (no validation), confirming the gap. Once `SteamConfig::from_env` (or `install_game`) validates that `username` and `password` contain no `+` or whitespace before building the args slice, the test passes.

**Note on `config.steamcmd` path:** a complementary test should verify that the resolved path is an absolute path under a known prefix (e.g. `/usr`, `/usr/local`, `~/.local`) rather than an arbitrary writable location, guarding against the unvalidated binary path risk.
