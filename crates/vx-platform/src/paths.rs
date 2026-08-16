//! Where the game keeps its files on disk.
//!
//! Follows the XDG Base Directory spec on Linux, which is the first-class
//! target: config in `$XDG_CONFIG_HOME`, saves and mods under
//! `$XDG_DATA_HOME`, both with the documented fallbacks.

use std::path::PathBuf;

/// Directory name used under the XDG roots.
const APP_DIR: &str = "gamingg";

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        // No HOME at all is pathological; keep going in the working directory
        // rather than panicking on startup.
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `$XDG_CONFIG_HOME/gamingg`, falling back to `~/.config/gamingg`.
pub fn config_dir() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

/// `$XDG_DATA_HOME/gamingg`, falling back to `~/.local/share/gamingg`.
pub fn data_dir() -> PathBuf {
    xdg_dir("XDG_DATA_HOME", ".local/share")
}

/// Where worlds are stored.
pub fn saves_dir() -> PathBuf {
    data_dir().join("saves")
}

/// Where locally installed mods are read from.
///
/// Steam Workshop mods live elsewhere — under Steam's own content directory —
/// and are discovered through a separate mod source. Both yield the same kind
/// of handle to the loader.
pub fn mods_dir() -> PathBuf {
    data_dir().join("mods")
}

/// Resolve an XDG variable, ignoring it when it is empty or relative, as the
/// spec requires.
fn xdg_dir(variable: &str, fallback: &str) -> PathBuf {
    let base = std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home().join(fallback));
    base.join(APP_DIR)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_path_ends_in_the_app_directory() {
        // Whatever the environment, these must be namespaced to the game and
        // never point at a shared root.
        for path in [config_dir(), data_dir()] {
            assert!(
                path.ends_with(APP_DIR),
                "{} is not namespaced to {APP_DIR}",
                path.display()
            );
        }
    }

    #[test]
    fn saves_and_mods_live_under_the_data_directory() {
        assert!(saves_dir().starts_with(data_dir()));
        assert!(mods_dir().starts_with(data_dir()));
        assert!(saves_dir().ends_with("saves"));
        assert!(mods_dir().ends_with("mods"));
    }

    #[test]
    fn config_and_data_are_distinct() {
        // Writing worlds into the config directory would be wrong even though
        // both resolve under the home directory by default.
        assert_ne!(config_dir(), data_dir());
    }

    #[test]
    fn paths_are_absolute_given_a_normal_environment() {
        if std::env::var_os("HOME").is_some() {
            assert!(config_dir().is_absolute());
            assert!(data_dir().is_absolute());
        }
    }
}
