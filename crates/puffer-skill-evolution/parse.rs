//! Parse a `SKILL.md` blob into a `SkillCandidate`.

use crate::{SkillCandidate, SkillFrontmatter};
use anyhow::{anyhow, Context, Result};

/// Parses a SKILL.md document into frontmatter and body.
///
/// The document must start with a YAML frontmatter block delimited by `---`.
/// Required fields are `name` and `description`.
pub fn parse_skill_md(text: &str) -> Result<SkillCandidate> {
    let trimmed = text.trim_start();
    let rest = trimmed
        .strip_prefix("---\n")
        .or_else(|| trimmed.strip_prefix("---\r\n"))
        .ok_or_else(|| anyhow!("missing opening --- delimiter"))?;
    let (frontmatter_text, body) = split_frontmatter_body(rest)?;
    let frontmatter: SkillFrontmatter = serde_yaml::from_str(frontmatter_text)
        .context("parsing skill frontmatter as YAML")?;

    validate_frontmatter(&frontmatter)?;

    Ok(SkillCandidate {
        frontmatter,
        body: body.trim_start().to_string(),
        scores: None,
    })
}

fn split_frontmatter_body(rest: &str) -> Result<(&str, &str)> {
    if let Some(end) = rest.find("\n---\n") {
        return Ok((&rest[..end], &rest[end + "\n---\n".len()..]));
    }
    if let Some(end) = rest.find("\n---\r\n") {
        return Ok((&rest[..end], &rest[end + "\n---\r\n".len()..]));
    }
    Err(anyhow!("missing closing --- delimiter"))
}

fn validate_frontmatter(frontmatter: &SkillFrontmatter) -> Result<()> {
    if frontmatter.name.is_empty() {
        return Err(anyhow!("frontmatter `name` is empty"));
    }
    if frontmatter.name.len() > 64 {
        return Err(anyhow!("frontmatter `name` exceeds 64 chars"));
    }
    if !frontmatter
        .name
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(anyhow!(
            "frontmatter `name` must contain only lowercase ASCII, digits, or hyphens"
        ));
    }
    if frontmatter.description.len() > 1024 {
        return Err(anyhow!("frontmatter `description` exceeds 1024 chars"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_skill() {
        let doc = "---\nname: my-skill\ndescription: Use when debugging X\n---\nBody text here.";
        let candidate = parse_skill_md(doc).unwrap();
        assert_eq!(candidate.frontmatter.name, "my-skill");
        assert_eq!(
            candidate.frontmatter.description,
            "Use when debugging X"
        );
        assert_eq!(candidate.body, "Body text here.");
    }

    #[test]
    fn parse_missing_opening_delimiter() {
        let doc = "name: my-skill\n---\nBody";
        assert!(parse_skill_md(doc).is_err());
    }

    #[test]
    fn parse_missing_closing_delimiter() {
        let doc = "---\nname: my-skill\nBody";
        assert!(parse_skill_md(doc).is_err());
    }

    #[test]
    fn parse_empty_name_rejected() {
        let doc = "---\nname: \"\"\ndescription: x\n---\nBody";
        assert!(parse_skill_md(doc).is_err());
    }

    #[test]
    fn parse_long_name_rejected() {
        let long = "a".repeat(65);
        let doc = format!("---\nname: {}\ndescription: x\n---\nBody", long);
        assert!(parse_skill_md(&doc).is_err());
    }
}
