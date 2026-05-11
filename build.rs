fn main() {
    // Recompile when frontend assets change (rust-embed doesn't track these)
    println!("cargo:rerun-if-changed=frontend/");
}
