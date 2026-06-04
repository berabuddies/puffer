use super::*;
use std::collections::HashSet;

#[test]
fn all_env_vars_have_unique_names() {
    let mut seen: HashSet<&'static str> = HashSet::new();
    for entry in ALL_ENV_VARS {
        assert!(
            seen.insert(entry.name),
            "duplicate env var name in registry: {}",
            entry.name
        );
    }
}

#[test]
fn all_env_vars_have_descriptions() {
    for entry in ALL_ENV_VARS {
        assert!(
            !entry.description.trim().is_empty(),
            "empty description for env var: {}",
            entry.name
        );
        // Names are uppercase identifiers (allow digits, underscores, and
        // parentheses for `PROGRAMFILES(X86)`).
        assert!(
            entry.name.chars().all(|c| c.is_ascii_uppercase()
                || c.is_ascii_digit()
                || c == '_'
                || c == '('
                || c == ')'),
            "non-uppercase env var name: {}",
            entry.name
        );
    }
}
