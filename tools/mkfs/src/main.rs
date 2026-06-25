//! Build the Phase 6c filesystem image from the real knowledge base and write
//! it to the path given as the first argument. Each `knowledge-base/entries/
//! *.md` file is packed under its id (e.g. "KB-0005"); the in-kernel loader
//! enumerates the directory and parses each entry's frontmatter.

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
        let id = path.file_stem().unwrap().to_string_lossy().into_owned();
        let bytes = fs::read(&path).expect("read entry");
        files.push((id, bytes));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0)); // deterministic directory order
    assert!(!files.is_empty(), "no KB entries found in {}", dir.display());

    let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let img = mkfs::build_image(&refs);
    fs::write(&out, &img).expect("write image");
    eprintln!(
        "mkfs: wrote {} ({} bytes); packed {} KB entries",
        out,
        img.len(),
        refs.len()
    );
}
