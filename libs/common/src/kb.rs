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
                "playbook" => in_playbook = true,
                _ => {}
            }
        }
    }
    Some(KbRecord { id: id?, title: title?, playbook: playbook?, match_cause })
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
    put("playbook:\n")?;
    put("  - \"")?; put(playbook)?; put("\"\n")?;
    put("---\n")?;
    Some(n)
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
