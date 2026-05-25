use anyhow::{anyhow, Result};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

/// Concrete input pattern compiled from a Lambda Skill host catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LambdaInputPattern {
    Exact(Value),
    Arg(String),
    SkillPath(String),
    Template(String),
    Object(BTreeMap<String, LambdaInputPattern>),
    Array(Vec<LambdaInputPattern>),
}

impl LambdaInputPattern {
    pub(super) fn from_json(value: Value) -> Result<Self> {
        match value {
            Value::Object(mut object) => {
                if object.len() == 1 {
                    if let Some(arg) = object.remove("$arg") {
                        let Some(arg) = arg.as_str() else {
                            return Err(anyhow!("$arg contract must be a string"));
                        };
                        return Ok(Self::Arg(arg.to_string()));
                    }
                    if let Some(path) = object.remove("$skill_path") {
                        let Some(path) = path.as_str() else {
                            return Err(anyhow!("$skill_path contract must be a string"));
                        };
                        validate_skill_path(path)?;
                        return Ok(Self::SkillPath(path.to_string()));
                    }
                    if let Some(template) = object.remove("$template") {
                        let Some(template) = template.as_str() else {
                            return Err(anyhow!("$template contract must be a string"));
                        };
                        return Ok(Self::Template(template.to_string()));
                    }
                }
                object
                    .into_iter()
                    .map(|(key, value)| Ok((key, Self::from_json(value)?)))
                    .collect::<Result<BTreeMap<_, _>>>()
                    .map(Self::Object)
            }
            Value::Array(items) => items
                .into_iter()
                .map(Self::from_json)
                .collect::<Result<Vec<_>>>()
                .map(Self::Array),
            other => Ok(Self::Exact(other)),
        }
    }

    pub(super) fn collect_arg_refs(&self, out: &mut BTreeSet<String>) {
        match self {
            Self::Arg(name) => {
                out.insert(name.clone());
            }
            Self::Template(template) => {
                collect_template_arg_refs(template, out);
            }
            Self::Object(object) => {
                for value in object.values() {
                    value.collect_arg_refs(out);
                }
            }
            Self::Array(items) => {
                for item in items {
                    item.collect_arg_refs(out);
                }
            }
            Self::Exact(_) | Self::SkillPath(_) => {}
        }
    }

    pub(super) fn matches(
        &self,
        args: &Map<String, Value>,
        skill_root: Option<&Path>,
        input: &Value,
    ) -> bool {
        match self {
            Self::Exact(expected) => expected == input,
            Self::Arg(name) => args.get(name) == Some(input),
            Self::SkillPath(relative) => input.as_str().is_some_and(|actual| {
                skill_root.is_some_and(|root| root.join(relative).display().to_string() == actual)
            }),
            Self::Template(template) => input.as_str().is_some_and(|text| {
                render_template(template, args).is_some_and(|expected| expected == text)
            }),
            Self::Object(pattern) => {
                let Some(object) = input.as_object() else {
                    return false;
                };
                object.len() == pattern.len()
                    && pattern.iter().all(|(key, pattern)| {
                        object
                            .get(key)
                            .is_some_and(|value| pattern.matches(args, skill_root, value))
                    })
            }
            Self::Array(pattern) => {
                let Some(items) = input.as_array() else {
                    return false;
                };
                items.len() == pattern.len()
                    && pattern
                        .iter()
                        .zip(items)
                        .all(|(pattern, value)| pattern.matches(args, skill_root, value))
            }
        }
    }
}

fn validate_skill_path(path: &str) -> Result<()> {
    let relative = Path::new(path);
    if path.trim().is_empty() || relative.is_absolute() {
        return Err(anyhow!(
            "$skill_path contract must be a non-empty relative path"
        ));
    }
    if relative.components().any(|part| {
        matches!(
            part,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(anyhow!(
            "$skill_path contract cannot escape the skill directory"
        ));
    }
    Ok(())
}

fn collect_template_arg_refs(template: &str, out: &mut BTreeSet<String>) {
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        rest = &rest[start + 2..];
        let Some(end) = rest.find('}') else {
            return;
        };
        if let Some((_, name)) = template_placeholder(&rest[..end]) {
            out.insert(name.to_string());
        }
        rest = &rest[end + 1..];
    }
}

fn render_template(template: &str, args: &Map<String, Value>) -> Option<String> {
    let mut output = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        rest = &rest[start + 2..];
        let end = rest.find('}')?;
        let placeholder = &rest[..end];
        let (format, name) = template_placeholder(placeholder)?;
        let value = args.get(name)?;
        match format {
            TemplateFormat::Json => output.push_str(&serde_json::to_string(value).ok()?),
            TemplateFormat::Shell => output.push_str(&shell_quote_value(value)?),
            TemplateFormat::ShellJoin => output.push_str(&shell_quote_array(value)?),
            TemplateFormat::Raw => {
                if let Some(text) = value.as_str() {
                    output.push_str(text);
                } else {
                    output.push_str(&serde_json::to_string(value).ok()?);
                }
            }
        }
        rest = &rest[end + 1..];
    }
    output.push_str(rest);
    Some(output)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemplateFormat {
    Raw,
    Json,
    Shell,
    ShellJoin,
}

fn template_placeholder(placeholder: &str) -> Option<(TemplateFormat, &str)> {
    let trimmed = placeholder.trim();
    let (format, name) = if let Some(name) = trimmed.strip_prefix("json:") {
        (TemplateFormat::Json, name.trim())
    } else if let Some(name) = trimmed.strip_prefix("shell:") {
        (TemplateFormat::Shell, name.trim())
    } else if let Some(name) = trimmed.strip_prefix("shell_join:") {
        (TemplateFormat::ShellJoin, name.trim())
    } else {
        (TemplateFormat::Raw, trimmed)
    };
    (!name.is_empty()
        && name
            .chars()
            .all(|ch| ch == '_' || ch == '-' || ch.is_ascii_alphanumeric()))
    .then_some((format, name))
}

fn shell_quote_value(value: &Value) -> Option<String> {
    let text = if let Some(text) = value.as_str() {
        text.to_string()
    } else {
        serde_json::to_string(value).ok()?
    };
    Some(format!("'{}'", text.replace('\'', r#"'"'"'"#)))
}

fn shell_quote_array(value: &Value) -> Option<String> {
    let items = value.as_array()?;
    items
        .iter()
        .map(shell_quote_value)
        .collect::<Option<Vec<_>>>()
        .map(|quoted| quoted.join(" "))
}
