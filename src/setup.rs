// setup.rs — where noguisteam keeps its files, first-run setup, Steam login.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// File locations for one run.
#[derive(Debug, Clone, PartialEq)]
pub struct Paths {
    /// KEY=value settings file (API keys). Kept at mode 0600.
    pub env_file: PathBuf,
    /// SQLite cache of the library and wishlist.
    pub db:       PathBuf,
    /// Directory searched for `colors.json` before the XDG config dir.
    pub legacy:   Option<PathBuf>,
}

impl Paths {
    /// A project-root setup from before XDG support (`.env` / `steam_games.db`
    /// next to the checkout) keeps working; otherwise use XDG directories.
    pub fn resolve() -> Self {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let legacy = legacy_root(
            env("NOGUISTEAM_HOME").map(PathBuf::from),
            std::env::current_exe().ok(),
        );
        Self::from_parts(legacy, env("XDG_CONFIG_HOME"), env("XDG_DATA_HOME"), env("HOME"))
    }

    fn from_parts(
        legacy: Option<PathBuf>,
        xdg_config: Option<String>,
        xdg_data: Option<String>,
        home: Option<String>,
    ) -> Self {
        if let Some(root) = legacy {
            return Self {
                env_file: root.join(".env"),
                db:       root.join("steam_games.db"),
                legacy:   Some(root),
            };
        }
        let home = PathBuf::from(home.unwrap_or_default());
        let config = xdg_config.map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
        let data = xdg_data.map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
        Self {
            env_file: config.join("noguisteam").join("env"),
            db:       data.join("noguisteam").join("steam_games.db"),
            legacy:   None,
        }
    }
}

fn is_legacy_root(dir: &Path) -> bool {
    dir.join(".env").exists() || dir.join("steam_games.db").exists()
}

/// `NOGUISTEAM_HOME`, or a directory up to six levels above the binary (a
/// `target/release` build inside the checkout), holding an old-style setup.
fn legacy_root(noguisteam_home: Option<PathBuf>, exe: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(dir) = noguisteam_home.filter(|d| is_legacy_root(d)) {
        return Some(dir);
    }
    let exe = exe?;
    exe.ancestors().skip(1).take(6).find(|d| is_legacy_root(d)).map(Path::to_path_buf)
}

/// Settings template written on first run. Only STEAM_API_KEY is required.
pub const ENV_TEMPLATE: &str = "\
# noguisteam settings (KEY=value). This file is kept private (mode 0600).
#
# Required: Steam Web API key — https://steamcommunity.com/dev/apikey
STEAM_API_KEY=

# Optional: IsThereAnyDeal key for wishlist sales — https://isthereanydeal.com/apps/my/
ITAD_KEY=

# Optional: regional pricing for the wishlist (two-letter country code).
COUNTRY=US

# Optional: sync the library from Steam every time noguisteam starts.
AUTO_SYNC=0

# Read from the local Steam client when unset; set them to override.
# STEAM_ID=
# STEAM_USERNAME=
# STEAMCMD=steamcmd
";

/// Create `path` with the template (0600, parent dir 0700) if it does not
/// exist. Returns true when it was created.
pub fn write_env_template(path: &Path) -> Result<bool> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    if path.exists() {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
            .with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true).create_new(true).mode(0o600)
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    f.write_all(ENV_TEMPLATE.as_bytes())?;
    Ok(true)
}

/// Tighten an existing settings file to 0600: it holds API keys.
pub fn make_private(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.permissions().mode() & 0o077 != 0 {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
}

/// The account the local Steam client last logged in with, from
/// `~/.steam/steam/config/loginusers.vdf`: (SteamID64, account name).
pub fn local_steam_account() -> Option<(String, String)> {
    let home = std::env::var("HOME").ok()?;
    let text = std::fs::read_to_string(
        Path::new(&home).join(".steam/steam/config/loginusers.vdf"),
    ).ok()?;
    parse_loginusers(&text)
}

/// Pick the `MostRecent` user from loginusers.vdf, or the only one listed.
fn parse_loginusers(text: &str) -> Option<(String, String)> {
    // Each user block: "7656119…" { "AccountName" "name" … "MostRecent" "1" }
    let mut users: Vec<(String, String, bool)> = Vec::new();
    let mut current: Option<(String, String, bool)> = None;
    for line in text.lines() {
        let tokens: Vec<&str> = line.split('"').skip(1).step_by(2).collect();
        match tokens.as_slice() {
            [id] if id.len() == 17 && id.bytes().all(|b| b.is_ascii_digit()) => {
                if let Some(u) = current.take() { users.push(u); }
                current = Some((id.to_string(), String::new(), false));
            }
            [key, value] => {
                if let Some(u) = current.as_mut() {
                    if key.eq_ignore_ascii_case("AccountName") { u.1 = value.to_string(); }
                    if key.eq_ignore_ascii_case("MostRecent") && *value == "1" { u.2 = true; }
                }
            }
            _ => {}
        }
    }
    users.extend(current);
    users.retain(|u| !u.1.is_empty());
    let pick = match users.iter().position(|u| u.2) {
        Some(i) => users.swap_remove(i),
        None if users.len() == 1 => users.pop()?,
        None => return None,
    };
    Some((pick.0, pick.1))
}

/// `noguisteam login`: run steamcmd interactively once so it caches the
/// session. steamcmd asks for the password and Steam Guard code itself, so
/// the password is never stored or passed on a command line.
pub fn login(steamcmd: &Path, username: &str) -> Result<()> {
    println!("Logging in to steamcmd as {username}. It will ask for your password and Steam Guard code;");
    println!("after this, installs and uninstalls use the cached login.\n");
    let status = std::process::Command::new(steamcmd)
        .args(["+login", username, "+quit"])
        .status()
        .with_context(|| format!("could not run {} — is steamcmd installed?", steamcmd.display()))?;
    if !status.success() {
        bail!("steamcmd exited with {status}");
    }
    println!("\nDone. Start noguisteam to install games.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_paths_by_default() {
        let p = Paths::from_parts(None, None, None, Some("/home/u".into()));
        assert_eq!(p.env_file, PathBuf::from("/home/u/.config/noguisteam/env"));
        assert_eq!(p.db, PathBuf::from("/home/u/.local/share/noguisteam/steam_games.db"));
        let p = Paths::from_parts(None, Some("/c".into()), Some("/d".into()), Some("/home/u".into()));
        assert_eq!(p.env_file, PathBuf::from("/c/noguisteam/env"));
        assert_eq!(p.db, PathBuf::from("/d/noguisteam/steam_games.db"));
    }

    #[test]
    fn legacy_checkout_keeps_its_files() {
        let root = scratch("legacy");
        std::fs::write(root.join(".env"), "").unwrap();
        let exe = root.join("target/release/noguisteam");
        assert_eq!(legacy_root(None, Some(exe.clone())), Some(root.clone()));
        let p = Paths::from_parts(Some(root.clone()), None, None, Some("/home/u".into()));
        assert_eq!(p.env_file, root.join(".env"));
        assert_eq!(p.db, root.join("steam_games.db"));
        // NOGUISTEAM_HOME without old files is ignored.
        assert_eq!(legacy_root(Some(scratch("empty")), None), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn template_is_private_and_never_overwritten() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("tmpl");
        let path = dir.join("cfg/env");
        assert!(write_env_template(&path).unwrap());
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::write(&path, "STEAM_API_KEY=x\n").unwrap();
        assert!(!write_env_template(&path).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "STEAM_API_KEY=x\n");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        make_private(&path);
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::remove_dir_all(dir).unwrap();
    }

    const VDF: &str = r#""users"
{
	"76561198000000001"
	{
		"AccountName"		"old_alt"
		"PersonaName"		"Alt"
		"MostRecent"		"0"
	}
	"76561198000000002"
	{
		"AccountName"		"main_acct"
		"PersonaName"		"Main"
		"MostRecent"		"1"
	}
}
"#;

    #[test]
    fn loginusers_picks_most_recent() {
        assert_eq!(
            parse_loginusers(VDF),
            Some(("76561198000000002".into(), "main_acct".into())),
        );
    }

    #[test]
    fn loginusers_single_user_without_most_recent() {
        let one = VDF.split("\t\"76561198000000002\"").next().unwrap().to_string() + "}\n";
        assert_eq!(parse_loginusers(&one), Some(("76561198000000001".into(), "old_alt".into())));
        let two_none = VDF.replace("\"MostRecent\"\t\t\"1\"", "\"MostRecent\"\t\t\"0\"");
        assert_eq!(parse_loginusers(&two_none), None, "ambiguous: no guess");
        assert_eq!(parse_loginusers(""), None);
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("noguisteam-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}
