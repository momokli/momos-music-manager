//! Mapping from the build platform to the published artifact.

use super::verify::PlatformArtifact;

/// Artifact for the current platform, or `None` if the updater does not
/// support it yet.
pub fn current_artifact() -> Option<PlatformArtifact> {
    let binary_name = if cfg!(windows) {
        "momos-music-manager.exe"
    } else {
        "momos-music-manager"
    };
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some(PlatformArtifact {
            os_arch: "linux-x64".into(),
            ext: "tar.gz".into(),
            binary_name: binary_name.into(),
        }),
        ("linux", "aarch64") => Some(PlatformArtifact {
            os_arch: "linux-arm64".into(),
            ext: "tar.gz".into(),
            binary_name: binary_name.into(),
        }),
        ("windows", "x86_64") => Some(PlatformArtifact {
            os_arch: "windows-x64".into(),
            ext: "zip".into(),
            binary_name: binary_name.into(),
        }),
        ("windows", "aarch64") => Some(PlatformArtifact {
            os_arch: "windows-arm64".into(),
            ext: "zip".into(),
            binary_name: binary_name.into(),
        }),
        // macOS: universal DMG. The executable lives inside the .app bundle,
        // so v1 only downloads + verifies (no atomic swap).
        ("macos", _) => Some(PlatformArtifact {
            os_arch: "macos-universal".into(),
            ext: "dmg".into(),
            binary_name: "momos-music-manager".into(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_platforms_look_sane() {
        // The function reads the *runtime* platform, so on CI (linux x64) we
        // can at least assert it returns something.
        if let Some(a) = current_artifact() {
            assert!(a.os_arch.starts_with("linux-") || a.os_arch.contains("windows-") || a.os_arch == "macos-universal");
            assert!(!a.ext.is_empty());
            assert!(!a.binary_name.is_empty());
        }
    }
}
