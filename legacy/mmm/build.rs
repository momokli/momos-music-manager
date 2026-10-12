fn main() {
    // Recompile when frontend assets change (rust-embed doesn't track these)
    println!("cargo:rerun-if-changed=frontend/");

    // Versioning: CI injects the effective version via the MMM_VERSION env
    // var (see scripts/resolve-version.sh). Local builds fall back to the
    // Cargo.toml version (CARGO_PKG_VERSION is set by Cargo for build
    // scripts, so env!("MMM_VERSION") stays safe everywhere).
    let version = std::env::var("MMM_VERSION")
        .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string());
    println!("cargo:rustc-env=MMM_VERSION={version}");
    println!("cargo:rerun-if-env-changed=MMM_VERSION");
}
