//! User-profile memory: writes the onboarding profile (country + role) into a
//! delimited "managed block" inside puffer's user-level global memory files
//! (`~/.puffer/AGENTS.md` + `~/.puffer/CLAUDE.md`). The block is upserted
//! idempotently so re-running onboarding replaces it without clobbering any
//! other content in those files. puffer-core reads these files fresh each turn
//! (see crates/puffer-core/runtime/system_prompt.rs), so no daemon restart is
//! needed.

use std::path::Path;

const BEGIN: &str = "<!-- BEGIN momo-user-profile (managed by onboarding) -->";
const END: &str = "<!-- END momo-user-profile -->";

/// Collapse a user-supplied string to a single safe line: strip the HTML-comment
/// delimiters (so a malicious/accidental marker string can't break block
/// detection) and fold all whitespace (incl. newlines) into single spaces.
fn sanitize(s: &str) -> String {
    s.replace("<!--", " ")
        .replace("-->", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build the managed block from optional country/role. Returns `None` when both
/// are empty (after sanitize), signalling "nothing to write".
pub fn build_block(country: Option<&str>, role: Option<&str>) -> Option<String> {
    let country = country.map(sanitize).filter(|s| !s.is_empty());
    let role = role.map(sanitize).filter(|s| !s.is_empty());
    if country.is_none() && role.is_none() {
        return None;
    }
    let mut lines = vec![BEGIN.to_string(), "## About the user".to_string()];
    if let Some(c) = &country {
        lines.push(format!("- Lives in: {c}"));
    }
    if let Some(r) = &role {
        lines.push(format!("- Role / occupation: {r}"));
    }
    lines.push(END.to_string());
    Some(lines.join("\n"))
}

fn ensure_trailing_newline(mut s: String) -> String {
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Insert or replace the managed block in `existing`. If a well-formed block
/// (BEGIN before END) is present, its inclusive span is replaced; otherwise the
/// block is appended (preserving all existing content). Idempotent.
pub fn upsert_managed_block(existing: &str, block: &str) -> String {
    match (existing.find(BEGIN), existing.find(END)) {
        (Some(b), Some(e)) if e > b => {
            let end_idx = e + END.len();
            let mut out = String::with_capacity(existing.len() + block.len());
            out.push_str(&existing[..b]);
            out.push_str(block);
            out.push_str(&existing[end_idx..]);
            ensure_trailing_newline(out)
        }
        _ => {
            let mut out = existing.trim_end().to_string();
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(block);
            ensure_trailing_newline(out)
        }
    }
}

/// Write the profile into `<puffer_dir>/AGENTS.md` and `<puffer_dir>/CLAUDE.md`.
/// Creates `puffer_dir` if missing. Returns `Ok(false)` (no files touched) when
/// both fields are empty. Reads each file (empty if absent), upserts the block,
/// writes it back.
pub fn write_profile_files(
    puffer_dir: &Path,
    country: Option<&str>,
    role: Option<&str>,
) -> std::io::Result<bool> {
    let Some(block) = build_block(country, role) else {
        return Ok(false);
    };
    std::fs::create_dir_all(puffer_dir)?;
    for name in ["AGENTS.md", "CLAUDE.md"] {
        let path = puffer_dir.join(name);
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let next = upsert_managed_block(&existing, &block);
        std::fs::write(&path, next)?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_block_emits_both_bullets() {
        let block = build_block(Some("United States"), Some("Founder")).unwrap();
        assert!(block.starts_with(BEGIN));
        assert!(block.ends_with(END));
        assert!(block.contains("- Lives in: United States"));
        assert!(block.contains("- Role / occupation: Founder"));
    }

    #[test]
    fn build_block_omits_empty_field_and_returns_none_when_both_empty() {
        let only_role = build_block(Some("   "), Some("Engineer")).unwrap();
        assert!(!only_role.contains("Lives in"));
        assert!(only_role.contains("- Role / occupation: Engineer"));
        assert!(build_block(Some(""), None).is_none());
    }

    #[test]
    fn sanitize_strips_markers_and_newlines() {
        let block = build_block(None, Some("Founder\n<!-- END momo-user-profile -->")).unwrap();
        // Exactly one BEGIN and one END marker survive.
        assert_eq!(block.matches(BEGIN).count(), 1);
        assert_eq!(block.matches(END).count(), 1);
        // split_whitespace().join(" ") leaves exactly one space between tokens.
        assert!(block.contains("- Role / occupation: Founder END momo-user-profile"));
    }

    #[test]
    fn upsert_into_empty_creates_block_with_trailing_newline() {
        let block = build_block(Some("Japan"), None).unwrap();
        let out = upsert_managed_block("", &block);
        assert!(out.contains("- Lives in: Japan"));
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn upsert_appends_without_clobbering_existing() {
        let block = build_block(Some("Korea"), None).unwrap();
        let out = upsert_managed_block("# My notes\n\nkeep me\n", &block);
        assert!(out.contains("# My notes"));
        assert!(out.contains("keep me"));
        assert!(out.contains("- Lives in: Korea"));
    }

    #[test]
    fn upsert_replaces_existing_block_and_is_idempotent() {
        let first = build_block(Some("Japan"), Some("Designer")).unwrap();
        let with_other = format!("preamble\n\n{first}\n\ntrailer\n");
        let second = build_block(Some("Singapore"), Some("Investor")).unwrap();
        let once = upsert_managed_block(&with_other, &second);
        assert!(once.contains("- Lives in: Singapore"));
        assert!(!once.contains("Japan"));
        assert!(once.contains("preamble"));
        assert!(once.contains("trailer"));
        assert_eq!(once.matches(BEGIN).count(), 1);
        // Writing the same block again changes nothing.
        let twice = upsert_managed_block(&once, &second);
        assert_eq!(once, twice);
    }

    #[test]
    fn write_profile_files_writes_both_and_skips_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let puffer = dir.path().join(".puffer");

        let wrote = write_profile_files(&puffer, Some("China"), Some("Student")).unwrap();
        assert!(wrote);
        for name in ["AGENTS.md", "CLAUDE.md"] {
            let body = std::fs::read_to_string(puffer.join(name)).unwrap();
            assert!(body.contains("- Lives in: China"));
            assert!(body.contains("- Role / occupation: Student"));
        }

        let wrote_empty = write_profile_files(&puffer, None, Some("  ")).unwrap();
        assert!(!wrote_empty);
    }
}
