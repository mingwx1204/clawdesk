//! Canonical user-scoped runtime path resolution for Codewhale.
//!
//! This leaf crate owns only the environment and platform-home decision. File
//! migration and per-subsystem fallback remain with the crate that owns those
//! files.
#![deny(missing_docs)]

use std::ffi::OsString;
use std::path::PathBuf;

/// Canonical Codewhale app directory name under the user home.
pub const CODEWHALE_APP_DIR: &str = ".codewhale";

/// Legacy DeepSeek-branded directory retained for compatibility reads.
pub const LEGACY_APP_DIR: &str = ".deepseek";

/// Return the explicit Codewhale home override, if one is configured.
///
/// Unicode values are trimmed so whitespace-only values are treated as unset,
/// matching the existing config and secret-store contract. Non-Unicode path
/// values are preserved on platforms that support them instead of silently
/// dropping an otherwise valid filesystem path.
#[must_use]
pub fn codewhale_home_override() -> Option<PathBuf> {
    path_env("CODEWHALE_HOME")
}

/// Whether `CODEWHALE_HOME` establishes an explicit isolation boundary.
#[must_use]
pub fn codewhale_home_is_explicit() -> bool {
    codewhale_home_override().is_some()
}

/// Return the legacy `DEEPSEEK_HOME` compatibility override, if configured.
///
/// New state must use [`codewhale_home`]. This resolver exists only for readers
/// whose persisted format still explicitly supports the legacy environment
/// alias.
#[must_use]
pub fn legacy_deepseek_home_override() -> Option<PathBuf> {
    path_env("DEEPSEEK_HOME")
}

/// Resolve the user's platform home, preferring `HOME` before `USERPROFILE`.
///
/// The explicit environment order makes CLI, state, config, and secret paths
/// deterministic in hermetic shells. On Windows, `HOMEDRIVE` plus `HOMEPATH`
/// remains a compatibility fallback before the platform resolver. The platform
/// resolver remains last for ordinary desktop launches without those variables.
#[must_use]
pub fn user_home() -> Option<PathBuf> {
    path_env("HOME")
        .or_else(|| path_env("USERPROFILE"))
        .or_else(windows_home_from_environment)
        .or_else(dirs::home_dir)
}

#[cfg(windows)]
fn windows_home_from_environment() -> Option<PathBuf> {
    let mut path = path_env("HOMEDRIVE")?;
    path.push(path_env("HOMEPATH")?);
    (!path.as_os_str().is_empty()).then_some(path)
}

#[cfg(not(windows))]
fn windows_home_from_environment() -> Option<PathBuf> {
    None
}

/// Resolve the canonical Codewhale runtime home.
///
/// An explicit `CODEWHALE_HOME` is returned verbatim. Otherwise this is
/// `<user home>/.codewhale`.
#[must_use]
pub fn codewhale_home() -> Option<PathBuf> {
    codewhale_home_override().or_else(|| user_home().map(|home| home.join(CODEWHALE_APP_DIR)))
}

/// Resolve the ambient legacy DeepSeek home used for compatibility reads.
///
/// This never follows `CODEWHALE_HOME`: callers must suppress legacy fallback
/// whenever [`codewhale_home_is_explicit`] is true.
#[must_use]
pub fn legacy_deepseek_home() -> Option<PathBuf> {
    user_home().map(|home| home.join(LEGACY_APP_DIR))
}

fn path_env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).and_then(normalize_path_value)
}

fn normalize_path_value(value: OsString) -> Option<PathBuf> {
    if value.is_empty() {
        return None;
    }
    match value.to_str() {
        Some(value) => {
            let value = value.trim();
            (!value.is_empty()).then(|| PathBuf::from(value))
        }
        None => Some(PathBuf::from(value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_path_values_are_trimmed_and_whitespace_is_unset() {
        assert_eq!(
            normalize_path_value(OsString::from("  /tmp/codewhale  ")),
            Some(PathBuf::from("/tmp/codewhale"))
        );
        assert_eq!(normalize_path_value(OsString::from(" \t\n ")), None);
        assert_eq!(normalize_path_value(OsString::new()), None);
    }

    #[cfg(unix)]
    #[test]
    fn unix_non_unicode_path_values_are_preserved() {
        use std::os::unix::ffi::OsStringExt;

        let value = OsString::from_vec(b"codewhale-\xff-home".to_vec());
        assert_eq!(
            normalize_path_value(value.clone()),
            Some(PathBuf::from(value))
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_non_unicode_path_values_are_preserved() {
        use std::os::windows::ffi::OsStringExt;

        let value = OsString::from_wide(&[b'C' as u16, b':' as u16, b'\\' as u16, 0xd800]);
        assert_eq!(
            normalize_path_value(value.clone()),
            Some(PathBuf::from(value))
        );
    }
}
