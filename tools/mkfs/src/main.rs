//! Build the Phase 6c filesystem image from the real knowledge base and write
//! it to the path given as the first argument. Only the **runtime-matchable**
//! entries are packed — those whose frontmatter declares a `match-cause` token
//! (today just KB-0005; KB-0001..0004 are dev-environment issues with no
//! runtime fault class). Each is packed under its id; the in-kernel loader
//! enumerates the directory and parses each entry's frontmatter. Filtering by
//! token (not by name) keeps both sides data-driven and the image small, so
//! the loader's boot-time reads stay few.

use std::{fs, path::PathBuf};

fn entries_dir() -> PathBuf {
    // tools/mkfs -> repo root is two levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../knowledge-base/entries")
}

fn main() {
    let out = std::env::args().nth(1).expect("usage: mkfs <image-path>");
    let dir = entries_dir();

    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for ent in fs::read_dir(&dir).expect("read knowledge-base/entries") {
        let path = ent.expect("dirent").path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let bytes = fs::read(&path).expect("read entry");
        // Pack only entries the self-healer can match at runtime (have a token).
        let matchable = kernel_common::kb::parse(&bytes)
            .map(|r| r.match_cause.is_some())
            .unwrap_or(false);
        if !matchable {
            continue;
        }
        let id = path.file_stem().unwrap().to_string_lossy().into_owned();
        files.push((id, bytes));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0)); // deterministic directory order
    assert!(
        !files.is_empty(),
        "no runtime-matchable KB entries (with a match-cause token) found in {}",
        dir.display()
    );

    let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let img = mkfs::build_image(&refs);
    fs::write(&out, &img).expect("write image");
    eprintln!(
        "mkfs: wrote {} ({} bytes); packed {} runtime-matchable KB entries",
        out,
        img.len(),
        refs.len()
    );
}
