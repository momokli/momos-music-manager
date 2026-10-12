//! Embed a git-derived build version so every build is traceable.
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    for k in ["MUSIC_API_VERSION", "GIT_SHA", "GIT_DESCRIBE"] {
        println!("cargo:rerun-if-env-changed={k}");
    }
    let pkg = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let sha = std::env::var("GIT_SHA")
        .ok()
        .or_else(|| git(&["rev-parse", "--short=8", "HEAD"]));
    let describe = std::env::var("GIT_DESCRIBE")
        .ok()
        .or_else(|| git(&["describe", "--tags", "--always", "--dirty"]));
    let version = std::env::var("MUSIC_API_VERSION")
        .ok()
        .unwrap_or_else(|| match &sha {
            Some(s) => format!("{pkg}-dev+{s}"),
            None => pkg.clone(),
        });
    println!("cargo:rustc-env=BUILD_VERSION={version}");
    println!(
        "cargo:rustc-env=BUILD_GIT_SHA={}",
        sha.unwrap_or_else(|| "unknown".into())
    );
    println!(
        "cargo:rustc-env=BUILD_GIT_DESCRIBE={}",
        describe.unwrap_or_else(|| "unknown".into())
    );
}
