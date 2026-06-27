//! Minimal parser for the runtime subset of a knowledge-base entry's YAML
//! frontmatter (see `knowledge-base/schema/issue-record.md`). Pure, `no_std`,
//! no allocation, no I/O — host-tested and used by the in-kernel KB loader.
//! It is deliberately not a general YAML parser: it reads the few scalar
//! fields the self-healer needs and skips everything else.

/// The runtime-relevant fields of one `knowledge-base/entries/*.md` record,
/// borrowed from the source bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KbRecord<'a> {
    pub id: &'a str,
    pub title: &'a str,
    /// The first playbook step — the actionable line.
    pub playbook: &'a str,
    /// The machine-matchable fault token, if the entry declares one.
    pub match_cause: Option<&'a str>,
    /// How many times the organism has diagnosed this issue (a fixed-width
    /// on-disk counter; `0` if the entry declares none).
    pub seen: u32,
}

/// Trim whitespace and strip a single pair of surrounding ASCII double quotes.
fn clean(v: &str) -> &str {
    let v = v.trim();
    let v = v.strip_prefix('"').unwrap_or(v);
    v.strip_suffix('"').unwrap_or(v).trim()
}

/// Parse the frontmatter of a KB entry. Returns `None` unless `id`, `title`,
/// and a first `playbook` step are all present, or if the file does not open
/// with a `---` fence. `match_cause` is optional.
pub fn parse(bytes: &[u8]) -> Option<KbRecord<'_>> {
    let text = core::str::from_utf8(bytes).ok()?;
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None; // must open with a frontmatter fence
    }
    let mut id = None;
    let mut title = None;
    let mut playbook = None;
    let mut match_cause = None;
    let mut seen = 0u32;
    let mut in_playbook = false;
    for line in lines {
        if line.trim() == "---" {
            break; // end of frontmatter
        }
        // Capture the first list item under `playbook:`.
        if in_playbook && playbook.is_none() {
            if let Some(item) = line.trim_start().strip_prefix("- ") {
                playbook = Some(clean(item));
                continue;
            }
        }
        if let Some((key, value)) = line.split_once(':') {
            match key.trim() {
                "id" => id = Some(clean(value)),
                "title" => title = Some(clean(value)),
                "match-cause" => match_cause = Some(clean(value)),
                "seen" => seen = clean(value).parse().unwrap_or(0),
                "playbook" => in_playbook = true,
                _ => {}
            }
        }
    }
    Some(KbRecord { id: id?, title: title?, playbook: playbook?, match_cause, seen })
}

/// Emit a KB entry document that `parse` round-trips, into `out`. Returns the
/// byte length written, or `None` if `out` is too small. The inverse of
/// `parse` for the runtime fields — so what the self-healer writes to disk is
/// provably what a later boot reads back. Strings are emitted double-quoted
/// (matching the schema and what `parse`'s `clean` strips); callers pass
/// already-sane ASCII values (no embedded quotes/newlines).
pub fn serialize(id: &str, title: &str, playbook: &str, match_cause: &str, out: &mut [u8]) -> Option<usize> {
    let mut n = 0usize;
    let mut put = |s: &str| -> Option<()> {
        let b = s.as_bytes();
        if n + b.len() > out.len() {
            return None;
        }
        out[n..n + b.len()].copy_from_slice(b);
        n += b.len();
        Some(())
    };
    put("---\n")?;
    put("id: ")?; put(id)?; put("\n")?;
    put("title: \"")?; put(title)?; put("\"\n")?;
    put("match-cause: ")?; put(match_cause)?; put("\n")?;
    put("seen: 00000\n")?;
    put("playbook:\n")?;
    put("  - \"")?; put(playbook)?; put("\"\n")?;
    put("---\n")?;
    Some(n)
}

/// Width of the fixed `seen: NNNNN` counter field. Fixed width is what makes an
/// in-place update a same-length byte overwrite (no shifting the rest of the
/// entry, no rewrite, no allocator).
pub const SEEN_WIDTH: usize = 5;

/// Overwrite the fixed-width `seen: NNNNN` counter in `block` with `count`
/// (zero-padded to `SEEN_WIDTH`). Returns `false` if the field is absent or its
/// value is not exactly `SEEN_WIDTH` ASCII digits — the in-place guard. Pure.
pub fn set_seen_in_block(block: &mut [u8], count: u32) -> bool {
    const KEY: &[u8] = b"seen: ";
    let Some(pos) = block.windows(KEY.len()).position(|w| w == KEY) else {
        return false;
    };
    let start = pos + KEY.len();
    let end = start + SEEN_WIDTH;
    if end > block.len() || !block[start..end].iter().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let mut n = count.min(99_999);
    for i in (0..SEEN_WIDTH).rev() {
        block[start + i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "---\n\
id: KB-0042\n\
title: \"A sample issue\"\n\
component: test\n\
match-cause: page-fault\n\
symptoms:\n\
  - \"It logs: something bad happened\"\n\
playbook:\n\
  - \"Do the first reversible thing\"\n\
  - \"Then the second\"\n\
verification: \"It works\"\n\
---\n\
\n## Notes\nfree text\n";

    #[test]
    fn parses_the_runtime_fields() {
        let r = parse(SAMPLE.as_bytes()).expect("parses");
        assert_eq!(r.id, "KB-0042");
        assert_eq!(r.title, "A sample issue");
        assert_eq!(r.playbook, "Do the first reversible thing");
        assert_eq!(r.match_cause, Some("page-fault"));
    }

    #[test]
    fn match_cause_is_optional() {
        let no_token = SAMPLE.replace("match-cause: page-fault\n", "");
        let r = parse(no_token.as_bytes()).expect("parses");
        assert_eq!(r.match_cause, None);
        assert_eq!(r.id, "KB-0042");
    }

    #[test]
    fn rejects_a_file_without_frontmatter() {
        assert!(parse(b"no frontmatter here").is_none());
    }

    #[test]
    fn serialize_round_trips_through_parse() {
        let mut buf = [0u8; 512];
        let n = serialize(
            "KB-0006",
            "Observed fault: illegal-instruction (auto-recorded)",
            "Restart the component, up to a bounded number of retries.",
            "illegal-instruction",
            &mut buf,
        )
        .expect("serializes within the buffer");
        let r = parse(&buf[..n]).expect("the emitted document parses");
        assert_eq!(r.id, "KB-0006");
        assert_eq!(r.title, "Observed fault: illegal-instruction (auto-recorded)");
        assert_eq!(r.playbook, "Restart the component, up to a bounded number of retries.");
        assert_eq!(r.match_cause, Some("illegal-instruction"));
    }

    #[test]
    fn parse_reads_seen_default_zero() {
        let r = parse(SAMPLE.as_bytes()).expect("parses");
        assert_eq!(r.seen, 0, "absent seen defaults to 0");
        let with = SAMPLE.replace("match-cause: page-fault\n", "match-cause: page-fault\nseen: 00042\n");
        assert_eq!(parse(with.as_bytes()).unwrap().seen, 42);
    }

    #[test]
    fn serialize_emits_a_parseable_seen() {
        let mut buf = [0u8; 512];
        let n = serialize("KB-0006", "t", "Restart.", "illegal-instruction", &mut buf).unwrap();
        assert_eq!(parse(&buf[..n]).unwrap().seen, 0);
    }

    #[test]
    fn set_seen_overwrites_in_place_and_round_trips() {
        let doc = "---\nid: KB-0005\ntitle: \"t\"\nmatch-cause: page-fault\nseen: 00000\nplaybook:\n  - \"Restart.\"\n---\n";
        let mut block = [0u8; 512];
        block[..doc.len()].copy_from_slice(doc.as_bytes());
        assert!(set_seen_in_block(&mut block, 7));
        assert_eq!(parse(&block).unwrap().seen, 7);
        assert!(set_seen_in_block(&mut block, 1234));
        assert_eq!(parse(&block).unwrap().seen, 1234);
    }

    #[test]
    fn set_seen_rejects_absent_or_malformed_field() {
        let mut no_field = [0u8; 64];
        let d = b"---\nid: KB-0001\nseen: bad\n---\n";
        no_field[..d.len()].copy_from_slice(d);
        assert!(!set_seen_in_block(&mut no_field, 3), "non-digit field rejected");
        let mut absent = [0u8; 32];
        absent[..16].copy_from_slice(b"---\nid: KB-0001\n");
        assert!(!set_seen_in_block(&mut absent, 3), "absent field rejected");
    }

    #[test]
    fn serialize_reports_none_when_buffer_too_small() {
        let mut tiny = [0u8; 8];
        assert!(serialize("KB-0006", "t", "p", "illegal-instruction", &mut tiny).is_none());
    }

    #[test]
    fn rejects_when_required_fields_missing() {
        // has id but no title/playbook
        assert!(parse(b"---\nid: KB-0001\n---\n").is_none());
    }

    #[test]
    fn parses_the_real_kb_0005() {
        let bytes = include_str!("../../../knowledge-base/entries/KB-0005.md");
        let r = parse(bytes.as_bytes()).expect("real KB-0005 parses");
        assert_eq!(r.id, "KB-0005");
        assert_eq!(r.match_cause, Some("page-fault"));
        assert!(r.playbook.starts_with("Restart the component"));
    }
}
