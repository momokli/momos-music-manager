//! Acceptance test for #518: the removed deemix sidecar must not creep back.
//!
//! `grep -ri deemix music-api/src music-api/deploy` must be empty **except** for
//! the single, marked legacy-env alias line (`legacy-env-alias`). This test
//! walks `src/` and `deploy/` so it runs identically locally and in CI, and
//! fails if a second occurrence appears (or the alias line disappears).

use std::path::{Path, PathBuf};

const MARKER: &str = "legacy-env-alias";

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[test]
fn no_deemix_anywhere_in_src_or_deploy() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    files_under(&root.join("src"), &mut files);
    files_under(&root.join("deploy"), &mut files);
    assert!(!files.is_empty(), "no files found under src/ or deploy/");

    let mut offenders = Vec::new();
    let mut marked = 0usize;
    for f in &files {
        // A file name carrying the brand is an offender too (the compose file).
        let name = f.file_name().unwrap().to_string_lossy().to_lowercase();
        if name.contains("deemix") {
            offenders.push(format!("filename: {}", f.display()));
        }
        let text = std::fs::read_to_string(f).unwrap_or_default();
        for (i, line) in text.lines().enumerate() {
            if line.to_lowercase().contains("deemix") {
                if line.contains(MARKER) {
                    marked += 1;
                } else {
                    offenders.push(format!("{}:{}: {}", f.display(), i + 1, line.trim()));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "deemix must be gone from src/ and deploy/ (#518); offenders:\n{}",
        offenders.join("\n")
    );
    assert_eq!(
        marked, 1,
        "exactly one marked exception line (the legacy ARL alias) is expected"
    );
}
