//! Build the Phase 6b demo filesystem image and write it to the path given as
//! the first argument. The demo file is named "KB-0005" and spans two blocks,
//! with a marker line near the end (in block 2) so the smoke test can prove a
//! multi-block extent read. (6c will generalize this to pack real KB files.)

use std::fs;

const DEMO_NAME: &str = "KB-0005";

fn demo_content() -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"PHASE 6B FILESYSTEM TEST FILE\n");
    while v.len() < 560 {
        v.extend_from_slice(b"....filler line....\n");
    }
    v.extend_from_slice(b"FS-6B-TAIL-OK\n");
    v
}

fn main() {
    let out = std::env::args().nth(1).expect("usage: mkfs <image-path>");
    let content = demo_content();
    let img = mkfs::build_image(&[(DEMO_NAME, &content)]);
    fs::write(&out, &img).expect("write image");
    eprintln!(
        "mkfs: wrote {} ({} bytes); file '{}' = {} bytes",
        out,
        img.len(),
        DEMO_NAME,
        content.len()
    );
}
