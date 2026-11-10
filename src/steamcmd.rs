// steamcmd.rs — steamcmd's own Steam directory.
//
// steamcmd and the Steam client used to share ~/.steam/steam. The client
// rewrites config/config.vdf when it starts and drops steamcmd's cached login,
// so every Steam session logged noguisteam out. steamcmd now runs with HOME
// pointing here; only the library's game folder (`steamapps/common`) is
// shared, and app manifests are copied across so the client sees installs.
//
// Layout (what the AUR steamcmd wrapper expects under $HOME):
//   <home>/.steam/root, steam   -> <home>/.steam
//   <home>/.steam/steamcmd      -> the user's ~/.steam/steamcmd (shared binary)
//   <home>/.steam/steamapps/common -> <library>/common
// The library's own steamapps/ is not linked: steamcmd writes its own
// libraryfolders.vdf there, which would replace the client's.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct CmdHome {
    /// Given to steamcmd as HOME.
    home: PathBuf,
}

impl CmdHome {
    pub fn new(home: PathBuf) -> Self {
        Self { home }
    }

    fn root(&self) -> PathBuf {
        self.home.join(".steam")
    }

    fn manifest(&self, app_id: u32) -> PathBuf {
        self.root().join("steamapps").join(format!("appmanifest_{app_id}.acf"))
    }

    /// Creates or repairs the layout; cheap enough to run before every steamcmd call.
    pub fn prepare(&self, steamlib: &Path, shared_steamcmd: Option<&Path>) -> Result<()> {
        let root = self.root();
        for dir in ["config", "steamapps"] {
            std::fs::create_dir_all(root.join(dir))
                .with_context(|| format!("could not create {}", root.join(dir).display()))?;
        }
        link(&root, &root.join("root"))?;
        link(&root, &root.join("steam"))?;
        if let Some(bin) = shared_steamcmd {
            link(bin, &root.join("steamcmd"))?;
        }
        let common = steamlib.join("common");
        std::fs::create_dir_all(&common)
            .with_context(|| format!("could not create {}", common.display()))?;
        link(&common, &root.join("steamapps/common"))
    }

    /// A steamcmd command that uses this directory instead of the client's.
    pub fn command(&self, steamcmd: &Path) -> Command {
        let mut c = Command::new(steamcmd);
        c.env("HOME", &self.home);
        c
    }

    /// After an install: copy steamcmd's app manifest into the library so the
    /// Steam client lists the game. False when steamcmd wrote none.
    pub fn adopt_manifest(&self, app_id: u32, steamlib: &Path) -> Result<bool> {
        let ours = self.manifest(app_id);
        if !ours.exists() {
            return Ok(false);
        }
        let theirs = steamlib.join(format!("appmanifest_{app_id}.acf"));
        std::fs::copy(&ours, &theirs)
            .with_context(|| format!("could not copy the app manifest to {}", theirs.display()))?;
        Ok(true)
    }

    /// Before an uninstall: steamcmd only uninstalls apps it has a manifest
    /// for, so give it the library's (the client may have installed the game).
    pub fn lend_manifest(&self, app_id: u32, steamlib: &Path) -> Result<()> {
        let theirs = steamlib.join(format!("appmanifest_{app_id}.acf"));
        if !theirs.exists() {
            return Ok(());
        }
        std::fs::copy(&theirs, self.manifest(app_id))
            .with_context(|| format!("could not copy {}", theirs.display()))?;
        Ok(())
    }
}

/// Makes `at` a symlink to `target`, replacing a stale link but never a real file or folder.
fn link(target: &Path, at: &Path) -> Result<()> {
    if at.is_symlink() {
        if std::fs::read_link(at).ok().as_deref() == Some(target) {
            return Ok(());
        }
        std::fs::remove_file(at)?;
    } else if at.exists() {
        anyhow::bail!("{} exists and is not a link to {}", at.display(), target.display());
    }
    std::os::unix::fs::symlink(target, at)
        .with_context(|| format!("could not link {} to {}", at.display(), target.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("noguisteam-cmd-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn setup(tag: &str) -> (PathBuf, CmdHome, PathBuf) {
        let d = scratch(tag);
        let lib = d.join("Steam/steamapps");
        fs::create_dir_all(&lib).unwrap();
        (d.clone(), CmdHome::new(d.join("cmdhome")), lib)
    }

    #[test]
    fn prepare_lays_out_a_steam_root_sharing_only_the_game_folder() {
        let (d, h, lib) = setup("layout");
        let bin = d.join("real-steamcmd");
        fs::create_dir_all(&bin).unwrap();
        h.prepare(&lib, Some(&bin)).unwrap();
        let root = d.join("cmdhome/.steam");
        assert_eq!(fs::read_link(root.join("root")).unwrap(), root);
        assert_eq!(fs::read_link(root.join("steam")).unwrap(), root);
        assert_eq!(fs::read_link(root.join("steamcmd")).unwrap(), bin);
        assert_eq!(fs::read_link(root.join("steamapps/common")).unwrap(), lib.join("common"));
        assert!(lib.join("common").is_dir(), "the library's game folder is created if missing");
        assert!(!root.join("steamapps").is_symlink(), "steamapps itself stays steamcmd's own");
        // Every run calls it again.
        h.prepare(&lib, Some(&bin)).unwrap();
    }

    #[test]
    fn without_a_shared_binary_the_wrapper_installs_its_own() {
        let (d, h, lib) = setup("nobin");
        h.prepare(&lib, None).unwrap();
        assert!(!d.join("cmdhome/.steam/steamcmd").exists());
    }

    #[test]
    fn steamcmd_runs_with_its_own_home() {
        let (d, h, _) = setup("cmd");
        let c = h.command(Path::new("steamcmd"));
        let home = c.get_envs().find(|(k, _)| *k == "HOME").and_then(|(_, v)| v);
        assert_eq!(home, Some(d.join("cmdhome").as_os_str()));
    }

    #[test]
    fn an_installed_manifest_is_copied_to_the_library() {
        let (d, h, lib) = setup("adopt");
        h.prepare(&lib, None).unwrap();
        assert!(!h.adopt_manifest(42, &lib).unwrap(), "nothing installed yet");
        fs::write(d.join("cmdhome/.steam/steamapps/appmanifest_42.acf"), "acf").unwrap();
        assert!(h.adopt_manifest(42, &lib).unwrap());
        assert_eq!(fs::read_to_string(lib.join("appmanifest_42.acf")).unwrap(), "acf");
    }

    #[test]
    fn the_library_manifest_is_lent_for_uninstall() {
        let (d, h, lib) = setup("lend");
        h.prepare(&lib, None).unwrap();
        fs::write(lib.join("appmanifest_42.acf"), "from client").unwrap();
        h.lend_manifest(42, &lib).unwrap();
        let ours = d.join("cmdhome/.steam/steamapps/appmanifest_42.acf");
        assert_eq!(fs::read_to_string(ours).unwrap(), "from client");
        // A game the library doesn't know about is not an error here.
        h.lend_manifest(7, &lib).unwrap();
    }
}
