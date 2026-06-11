fn main() {
    // Recompile when frontend assets change
    println!("cargo:rerun-if-changed=frontend/dist/");
    println!("cargo:rerun-if-changed=frontend/src/");
    println!("cargo:rerun-if-changed=frontend/index.html");

    // Attempt to build the frontend if dist/ is missing
    let dist = std::path::Path::new("frontend/dist/index.html");
    if !dist.exists() {
        println!(
            "cargo:warning=frontend/dist/ not found — run 'cd frontend && npm run build' first"
        );
    }
}
