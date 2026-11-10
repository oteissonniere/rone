//! Install detection for `rone upgrade`: the pure classification rule used to
//! decide whether the running binary came from the installer script or from a
//! Homebrew formula.

use rone::{InstallKind, classify_install_kind};

#[test]
fn script_installs_are_classified() {
    assert_eq!(
        classify_install_kind("/Users/olivier/.local/share/rone/v0.2.0/bin"),
        Some(InstallKind::Script),
    );
    assert_eq!(
        classify_install_kind("/home/olivier/.local/share/rone/v0.1.3/bin"),
        Some(InstallKind::Script),
    );
}

#[test]
fn brew_installs_are_classified() {
    // Apple Silicon and Intel Homebrew prefixes.
    assert_eq!(
        classify_install_kind("/opt/homebrew/Cellar/rone/0.2.0/bin"),
        Some(InstallKind::Brew),
    );
    assert_eq!(
        classify_install_kind("/usr/local/Cellar/rone/0.1.3/bin"),
        Some(InstallKind::Brew),
    );
}

#[test]
fn unknown_locations_are_not_classified() {
    assert_eq!(
        classify_install_kind("/Users/x/projects/rone/target/release"),
        None,
    );
    assert_eq!(classify_install_kind("/usr/bin"), None);
}
