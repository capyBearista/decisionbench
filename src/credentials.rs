//! Per-user API keys: `credentials.toml` beside the config file.

use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::batch::{resolve_links, write_atomic};
use crate::provider::Provider;

/// The saved keys, one per provider name. No `Debug`: a stray `{:?}` must not print a key.
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openrouter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zen: Option<String>,
}

impl Credentials {
    pub fn get(&self, provider: Provider) -> Option<&str> {
        match provider {
            Provider::OpenRouter => self.openrouter.as_deref(),
            Provider::Zen => self.zen.as_deref(),
        }
    }
}

/// The credentials file beside `config`, so `--config` moves both.
pub fn path_for(config: &Path) -> PathBuf {
    config.with_file_name("credentials.toml")
}

/// A missing file holds no keys; an unreadable or invalid one is an error. Errors carry the
/// parser's message and key names only, never a quoted source line or a value, which could be a key.
pub fn load(path: &Path) -> Result<Credentials, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Credentials::default()),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let invalid = |message: &str| format!("invalid credentials {}: {message}", path.display());
    let table: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| invalid(e.message()))?;
    // A wrong-typed value would be echoed by the type error, so name only its key.
    if let Some((name, _)) = table.iter().find(|(_, value)| !value.is_str()) {
        return Err(invalid(&format!("`{name}` must be a quoted string")));
    }
    table
        .try_into()
        .map_err(|e: toml::de::Error| invalid(e.message()))
}

/// Saves `key` for `provider`, keeping the other provider's key. The file is always private (0600).
pub fn save_key(path: &Path, provider: Provider, key: &str) -> Result<(), String> {
    // A symlinked file is written through, so the link survives the rename.
    let path = resolve_links(path)?;
    let mut credentials = load(&path)?;
    match provider {
        Provider::OpenRouter => credentials.openrouter = Some(key.to_string()),
        Provider::Zen => credentials.zen = Some(key.to_string()),
    }
    let text = toml::to_string(&credentials).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    write_atomic(&path, &text, Permissions::from_mode(0o600))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::batch::temp_files_in;
    use crate::provider::Provider;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    /// A fresh directory that does not exist yet.
    fn dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "decisionbench-cred-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn file(text: &str) -> PathBuf {
        let dir = dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("credentials.toml");
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn the_path_is_a_sibling_of_the_config() {
        assert_eq!(
            path_for(Path::new("/a/b/config.toml")),
            Path::new("/a/b/credentials.toml")
        );
        assert_eq!(
            path_for(Path::new("mine.toml")),
            Path::new("credentials.toml")
        );
    }

    #[test]
    fn a_missing_file_holds_no_keys() {
        let credentials = load(&dir().join("credentials.toml")).unwrap();
        assert!(credentials.openrouter.is_none() && credentials.zen.is_none());
    }

    #[test]
    fn both_keys_load() {
        let credentials = load(&file("openrouter = \"or-1\"\nzen = \"zen-1\"\n")).unwrap();
        assert_eq!(credentials.openrouter.as_deref(), Some("or-1"));
        assert_eq!(credentials.zen.as_deref(), Some("zen-1"));
    }

    #[test]
    fn bad_files_are_errors_that_name_the_path_and_never_echo_a_value() {
        for text in [
            "typo = \"sk-secret-value\"\n",
            "openrouter = [\"sk-secret-value\"]\n",
            "openrouter = sk-secret-value\n",
            "openrouter = 12345678\n",
            "openrouter = true\n",
            "openrouter = \"sk-secret-value\"\nopenrouter = \"sk-secret-value\"\n",
        ] {
            let path = file(text);
            let error = load(&path).err().expect(text);
            assert!(error.contains(&path.display().to_string()), "{error}");
            assert!(!error.contains("sk-secret-value"), "{error}");
        }
        let error = load(&file("typo = \"x\"\n")).err().unwrap();
        assert!(error.contains("typo"), "{error}");
        let error = load(&file("openrouter = 12345678\n")).err().unwrap();
        assert!(
            error.contains("`openrouter` must be a quoted string"),
            "{error}"
        );
    }

    #[test]
    fn an_unreadable_file_is_an_error_not_an_absent_one() {
        let path = file("");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let error = load(&path).err().unwrap();
        assert!(
            error.starts_with(&format!("cannot read {}", path.display())),
            "{error}"
        );
    }

    #[test]
    fn saving_creates_a_private_file_and_makes_the_parent_directory() {
        let path = dir().join("nested/credentials.toml");
        save_key(&path, Provider::Zen, "zen-1").unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(load(&path).unwrap().zen.as_deref(), Some("zen-1"));
        assert_eq!(temp_files_in(path.parent().unwrap()), Vec::<String>::new());
    }

    #[test]
    fn saving_makes_a_stale_world_readable_file_private() {
        let path = file("zen = \"old\"\n");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        save_key(&path, Provider::Zen, "new").unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(load(&path).unwrap().zen.as_deref(), Some("new"));
    }

    /// A symlink or stale file at the old fixed temp name must not receive the key or lend its mode.
    #[test]
    fn saving_ignores_a_planted_symlink_and_a_stale_world_readable_temp_file() {
        for plant_symlink in [true, false] {
            let path = file("zen = \"old\"\n");
            let dir = path.parent().unwrap();
            let (planted, victim) = (dir.join("credentials.toml.tmp"), dir.join("victim"));
            if plant_symlink {
                std::os::unix::fs::symlink(&victim, &planted).unwrap();
            } else {
                std::fs::write(&planted, "stale").unwrap();
                std::fs::set_permissions(&planted, std::fs::Permissions::from_mode(0o644)).unwrap();
            }
            save_key(&path, Provider::Zen, "new").unwrap();
            assert_eq!(mode(&path), 0o600);
            assert_eq!(load(&path).unwrap().zen.as_deref(), Some("new"));
            assert!(!victim.exists());
            if plant_symlink {
                assert!(std::fs::symlink_metadata(&planted).unwrap().is_symlink());
            } else {
                assert_eq!(std::fs::read_to_string(&planted).unwrap(), "stale");
                assert_eq!(mode(&planted), 0o644);
            }
            assert_eq!(temp_files_in(dir), ["credentials.toml.tmp"]);
        }
    }

    #[test]
    fn consecutive_saves_leave_no_temp_file() {
        let path = file("");
        save_key(&path, Provider::Zen, "zen-1").unwrap();
        save_key(&path, Provider::OpenRouter, "or-1").unwrap();
        assert_eq!(temp_files_in(path.parent().unwrap()), Vec::<String>::new());
    }

    #[test]
    fn saving_one_provider_keeps_the_other_and_replaces_its_own() {
        let path = dir().join("credentials.toml");
        save_key(&path, Provider::OpenRouter, "or-1").unwrap();
        save_key(&path, Provider::Zen, "zen-1").unwrap();
        save_key(&path, Provider::OpenRouter, "or-2").unwrap();
        let credentials = load(&path).unwrap();
        assert_eq!(credentials.openrouter.as_deref(), Some("or-2"));
        assert_eq!(credentials.zen.as_deref(), Some("zen-1"));
    }

    #[test]
    fn saving_follows_symlinks_and_keeps_the_link() {
        let root = dir();
        std::fs::create_dir_all(root.join("real")).unwrap();
        let (link, hop, target) = (
            root.join("credentials.toml"),
            root.join("hop.toml"),
            root.join("real/creds.toml"),
        );
        std::os::unix::fs::symlink("hop.toml", &link).unwrap();
        std::os::unix::fs::symlink("real/creds.toml", &hop).unwrap(); // dangling target
        save_key(&link, Provider::Zen, "zen-1").unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert!(std::fs::symlink_metadata(&hop).unwrap().is_symlink());
        assert_eq!(load(&target).unwrap().zen.as_deref(), Some("zen-1"));
        assert_eq!(mode(&target), 0o600);
    }

    #[test]
    fn an_invalid_existing_file_is_not_overwritten() {
        let path = file("typo = \"keep me\"\n");
        let error = save_key(&path, Provider::Zen, "zen-1").unwrap_err();
        assert!(error.contains(&path.display().to_string()), "{error}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "typo = \"keep me\"\n"
        );
    }
}
