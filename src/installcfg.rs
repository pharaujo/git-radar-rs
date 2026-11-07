//! Spawn-free lookup of the configuration files belonging to the `git` installation that's
//! first on `PATH`.
//!
//! gix can discover the *installation* config by running `git config -l --show-origin`,
//! but that is a subprocess on every prompt. Instead we `stat` our way to the binary and map
//! known layouts to their config files:
//!
//! * Apple's `/usr/bin/git` shim: system config `/etc/gitconfig`, plus the installation config
//!   `<developer dir>/usr/share/git-core/gitconfig`. The developer dir comes from `DEVELOPER_DIR`
//!   or the `/var/db/xcode_select_link` symlink.
//! * `<prefix>/bin/git` (Homebrew, source builds): system config `<prefix>/etc/gitconfig`.
//!   For a `/usr` prefix that is `/etc/gitconfig`, as distributions configure it.
//! * Anything else: no override, so gix falls back to `/etc/gitconfig`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Config file locations to hand to `gix::open::Options`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InstallCfg {
    /// Overrides the system config path. `None` keeps gix's default (`/etc/gitconfig`).
    pub system: Option<PathBuf>,
    /// The git installation config, if one exists. Only Apple git has a separate one.
    pub installation: Option<PathBuf>,
}

/// Git's boolean parsing for environment variables (`git_env_bool`).
pub fn env_truthy(value: Option<OsString>) -> bool {
    match value {
        None => false,
        Some(v) => {
            let v = v.to_string_lossy().to_ascii_lowercase();
            matches!(v.as_str(), "1" | "true" | "yes" | "on")
        }
    }
}

/// Look up the installation and system config of the first `git` on `path_var`.
///
/// `root` prefixes every absolute path we touch, so tests can run the lookup against a fake
/// filesystem tree. Production passes `/`.
pub fn lookup(
    path_var: Option<OsString>,
    env: &dyn Fn(&str) -> Option<OsString>,
    root: &Path,
) -> InstallCfg {
    if env_truthy(env("GIT_CONFIG_NOSYSTEM")) {
        return InstallCfg::default();
    }
    let Some(git) = first_git_on_path(path_var, root) else {
        return InstallCfg::default();
    };
    // `GIT_CONFIG_SYSTEM` replaces the system file. Let gix apply it by not overriding.
    let system_overridden_by_env = env("GIT_CONFIG_SYSTEM").is_some();
    let logical_dir = git.parent().unwrap_or(Path::new("/"));

    let mut cfg = if logical_dir == Path::new("/usr/bin") && is_apple(root) {
        InstallCfg {
            system: Some(rooted(root, "/etc/gitconfig")),
            installation: apple_installation_config(env, root),
        }
    } else if let Some(system) = prefix_system_config(&git, root) {
        InstallCfg {
            system: Some(system),
            installation: None,
        }
    } else {
        InstallCfg::default()
    };
    if system_overridden_by_env {
        cfg.system = None;
    }
    cfg
}

/// `root`-relative absolute path.
fn rooted(root: &Path, abs: impl AsRef<Path>) -> PathBuf {
    let abs = abs.as_ref();
    root.join(abs.strip_prefix("/").unwrap_or(abs))
}

/// Find the first executable `git` in `PATH`, returned as its *logical* (un-rooted) path.
fn first_git_on_path(path_var: Option<OsString>, root: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path_var = path_var?;
    std::env::split_paths(&path_var)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("git"))
        .find(|candidate| {
            std::fs::metadata(rooted(root, candidate))
                .is_ok_and(|md| md.is_file() && md.permissions().mode() & 0o111 != 0)
        })
}

fn is_apple(root: &Path) -> bool {
    // The shim only exists on macOS, where xcode-select manages the developer dir link.
    cfg!(target_os = "macos")
        || rooted(root, "/var/db/xcode_select_link")
            .symlink_metadata()
            .is_ok()
}

fn apple_installation_config(
    env: &dyn Fn(&str) -> Option<OsString>,
    root: &Path,
) -> Option<PathBuf> {
    let devdir = match env("DEVELOPER_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => std::fs::read_link(rooted(root, "/var/db/xcode_select_link")).ok()?,
    };
    [
        "usr/share/git-core/gitconfig",
        "Toolchains/XcodeDefault.xctoolchain/usr/share/git-core/gitconfig",
    ]
    .iter()
    .map(|rel| rooted(root, devdir.join(rel)))
    .find(|p| p.is_file())
}

/// For `<prefix>/bin/git`, return `<prefix>/etc/gitconfig`, trying the PATH location before the
/// canonicalized one. Homebrew's `/opt/homebrew/bin/git` is a symlink into the Cellar, but git
/// was built with `sysconfdir=/opt/homebrew/etc`.
fn prefix_system_config(git: &Path, root: &Path) -> Option<PathBuf> {
    fn for_bin_dir(bin: &Path) -> Option<PathBuf> {
        if bin.file_name()? != "bin" {
            return None;
        }
        let prefix = bin.parent()?;
        Some(if prefix == Path::new("/usr") {
            PathBuf::from("/etc/gitconfig")
        } else {
            prefix.join("etc/gitconfig")
        })
    }
    let logical = for_bin_dir(git.parent()?).map(|p| rooted(root, p));
    if let Some(p) = logical.as_ref().filter(|p| p.is_file()) {
        return Some(p.clone());
    }
    let from_canonical = (|| {
        let canonical_root = std::fs::canonicalize(root).ok()?;
        let canonical = std::fs::canonicalize(rooted(root, git)).ok()?;
        let canonical = Path::new("/").join(canonical.strip_prefix(&canonical_root).ok()?);
        for_bin_dir(canonical.parent()?).map(|p| rooted(root, p))
    })();
    match from_canonical.filter(|p| p.is_file()) {
        Some(p) => Some(p),
        // A recognized layout without an existing file still overrides the system path, so gix
        // doesn't read `/etc/gitconfig`, which this git ignores.
        None => logical,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn exe(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    fn file(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "[core]\n").unwrap();
    }
    fn no_env(_: &str) -> Option<OsString> {
        None
    }

    #[test]
    fn homebrew_layout() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        exe(&root.join("opt/homebrew/bin/git"));
        file(&root.join("opt/homebrew/etc/gitconfig"));
        let cfg = lookup(Some("/opt/homebrew/bin:/usr/bin".into()), &no_env, root);
        assert_eq!(cfg.system, Some(root.join("opt/homebrew/etc/gitconfig")));
        assert_eq!(cfg.installation, None);
    }

    #[test]
    fn symlink_into_cellar_prefers_path_prefix_then_canonical() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let real = root.join("opt/homebrew/Cellar/git/2.56.0/bin/git");
        exe(&real);
        std::fs::create_dir_all(root.join("opt/homebrew/bin")).unwrap();
        symlink(&real, root.join("opt/homebrew/bin/git")).unwrap();
        // Only the Cellar has a config file: fall back to the canonical prefix.
        file(&root.join("opt/homebrew/Cellar/git/2.56.0/etc/gitconfig"));
        let cfg = lookup(Some("/opt/homebrew/bin".into()), &no_env, root);
        let canonical_root = std::fs::canonicalize(root).unwrap();
        assert_eq!(
            cfg.system.map(|p| std::fs::canonicalize(p).unwrap()),
            Some(canonical_root.join("opt/homebrew/Cellar/git/2.56.0/etc/gitconfig"))
        );
        // Once the PATH prefix has one, it wins.
        file(&root.join("opt/homebrew/etc/gitconfig"));
        let cfg = lookup(Some("/opt/homebrew/bin".into()), &no_env, root);
        assert_eq!(cfg.system, Some(root.join("opt/homebrew/etc/gitconfig")));
    }

    #[test]
    fn recognized_layout_without_file_still_overrides_system() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        exe(&root.join("opt/homebrew/bin/git"));
        let cfg = lookup(Some("/opt/homebrew/bin".into()), &no_env, root);
        assert_eq!(cfg.system, Some(root.join("opt/homebrew/etc/gitconfig")));
    }

    #[test]
    fn apple_with_developer_dir() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        exe(&root.join("usr/bin/git"));
        file(&root.join("Dev/usr/share/git-core/gitconfig"));
        std::fs::create_dir_all(root.join("var/db")).unwrap();
        let env = |k: &str| (k == "DEVELOPER_DIR").then(|| OsString::from("/Dev"));
        let cfg = lookup(Some("/usr/bin".into()), &env, root);
        assert_eq!(
            cfg.installation,
            Some(root.join("Dev/usr/share/git-core/gitconfig"))
        );
        assert_eq!(cfg.system, Some(root.join("etc/gitconfig")));
    }

    #[test]
    fn apple_with_xcode_select_link() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        exe(&root.join("usr/bin/git"));
        file(&root.join("Library/Developer/CommandLineTools/usr/share/git-core/gitconfig"));
        std::fs::create_dir_all(root.join("var/db")).unwrap();
        symlink(
            "/Library/Developer/CommandLineTools",
            root.join("var/db/xcode_select_link"),
        )
        .unwrap();
        let cfg = lookup(Some("/usr/bin".into()), &no_env, root);
        assert_eq!(
            cfg.installation,
            Some(root.join("Library/Developer/CommandLineTools/usr/share/git-core/gitconfig"))
        );
    }

    #[test]
    fn apple_xcode_toolchain_location() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        exe(&root.join("usr/bin/git"));
        file(&root.join("X/Toolchains/XcodeDefault.xctoolchain/usr/share/git-core/gitconfig"));
        std::fs::create_dir_all(root.join("var/db")).unwrap();
        let env = |k: &str| (k == "DEVELOPER_DIR").then(|| OsString::from("/X"));
        let cfg = lookup(Some("/usr/bin".into()), &env, root);
        assert_eq!(
            cfg.installation,
            Some(root.join("X/Toolchains/XcodeDefault.xctoolchain/usr/share/git-core/gitconfig"))
        );
    }

    #[test]
    fn non_executable_and_missing_entries_are_skipped() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        file(&root.join("first/bin/git")); // not executable
        exe(&root.join("second/bin/git"));
        file(&root.join("second/etc/gitconfig"));
        let cfg = lookup(
            Some("/missing/bin:/first/bin:/second/bin".into()),
            &no_env,
            root,
        );
        assert_eq!(cfg.system, Some(root.join("second/etc/gitconfig")));
    }

    #[test]
    fn nosystem_disables_everything() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        exe(&root.join("opt/homebrew/bin/git"));
        file(&root.join("opt/homebrew/etc/gitconfig"));
        let env = |k: &str| (k == "GIT_CONFIG_NOSYSTEM").then(|| OsString::from("1"));
        assert_eq!(
            lookup(Some("/opt/homebrew/bin".into()), &env, root),
            InstallCfg::default()
        );
        let env = |k: &str| (k == "GIT_CONFIG_NOSYSTEM").then(|| OsString::from("0"));
        assert!(
            lookup(Some("/opt/homebrew/bin".into()), &env, root)
                .system
                .is_some()
        );
    }

    #[test]
    fn git_config_system_env_defers_to_gix() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        exe(&root.join("opt/homebrew/bin/git"));
        let env = |k: &str| (k == "GIT_CONFIG_SYSTEM").then(|| OsString::from("/x"));
        assert_eq!(
            lookup(Some("/opt/homebrew/bin".into()), &env, root).system,
            None
        );
    }

    #[test]
    fn unknown_layout_and_no_git() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        exe(&root.join("weird/place/git"));
        assert_eq!(
            lookup(Some("/weird/place".into()), &no_env, root),
            InstallCfg::default()
        );
        assert_eq!(
            lookup(Some("/nothing".into()), &no_env, root),
            InstallCfg::default()
        );
        assert_eq!(lookup(None, &no_env, root), InstallCfg::default());
    }
}
