use anyhow::{anyhow, Result};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

/// Concrete input pattern compiled from a Lambda Skill host catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LambdaInputPattern {
    Exact(Value),
    Arg(String),
    IntArg(String),
    SkillPath(String),
    Template(String),
    ShellJson(Box<LambdaInputPattern>),
    Concat(Vec<LambdaInputPattern>),
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
                    if let Some(arg) = object.remove("$int_arg") {
                        let Some(arg) = arg.as_str() else {
                            return Err(anyhow!("$int_arg contract must be a string"));
                        };
                        return Ok(Self::IntArg(arg.to_string()));
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
                        validate_template_placeholders(template)?;
                        return Ok(Self::Template(template.to_string()));
                    }
                    if let Some(shell_json) = object.remove("$shell_json") {
                        return Ok(Self::ShellJson(Box::new(Self::from_json(shell_json)?)));
                    }
                    if let Some(concat) = object.remove("$concat") {
                        let Value::Array(items) = concat else {
                            return Err(anyhow!("$concat contract must be an array"));
                        };
                        return items
                            .into_iter()
                            .map(Self::from_json)
                            .collect::<Result<Vec<_>>>()
                            .map(Self::Concat);
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
            Self::Arg(name) | Self::IntArg(name) => {
                out.insert(name.clone());
            }
            Self::Template(template) => {
                collect_template_arg_refs(template, out);
            }
            Self::ShellJson(pattern) => pattern.collect_arg_refs(out),
            Self::Concat(parts) => {
                for part in parts {
                    part.collect_arg_refs(out);
                }
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
        self.render_value(args, skill_root).as_ref() == Some(input)
    }

    pub(super) fn render_value(
        &self,
        args: &Map<String, Value>,
        skill_root: Option<&Path>,
    ) -> Option<Value> {
        match self {
            Self::Exact(expected) => Some(expected.clone()),
            Self::Arg(name) => args.get(name).cloned(),
            Self::IntArg(name) => render_int_arg(args.get(name)?),
            Self::SkillPath(relative) => {
                let root = skill_root?;
                Some(Value::String(root.join(relative).display().to_string()))
            }
            Self::Template(template) => {
                render_template(template, args, skill_root).map(Value::String)
            }
            Self::ShellJson(pattern) => {
                let value = pattern.render_value(args, skill_root)?;
                let json = serde_json::to_string(&value).ok()?;
                Some(Value::String(shell_quote_string(&json)))
            }
            Self::Concat(parts) => parts
                .iter()
                .map(|part| part.render_string(args, skill_root))
                .collect::<Option<Vec<_>>>()
                .map(|items| Value::String(items.concat())),
            Self::Object(pattern) => pattern
                .iter()
                .map(|(key, pattern)| Some((key.clone(), pattern.render_value(args, skill_root)?)))
                .collect::<Option<Map<String, Value>>>()
                .map(Value::Object),
            Self::Array(pattern) => pattern
                .iter()
                .map(|pattern| pattern.render_value(args, skill_root))
                .collect::<Option<Vec<_>>>()
                .map(Value::Array),
        }
    }

    fn render_string(
        &self,
        args: &Map<String, Value>,
        skill_root: Option<&Path>,
    ) -> Option<String> {
        match self.render_value(args, skill_root)? {
            Value::String(text) => Some(text),
            value => serde_json::to_string(&value).ok(),
        }
    }
}

fn render_int_arg(value: &Value) -> Option<Value> {
    match value {
        Value::Number(number) if number.is_i64() || number.is_u64() => {
            Some(Value::Number(number.clone()))
        }
        Value::String(text) => text
            .trim()
            .parse::<i64>()
            .ok()
            .map(|number| Value::Number(number.into())),
        _ => None,
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

fn validate_template_placeholders(template: &str) -> Result<()> {
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        rest = &rest[start + 2..];
        let Some(end) = rest.find('}') else {
            return Ok(());
        };
        if let Some(placeholder) = template_placeholder(&rest[..end]) {
            if matches!(
                placeholder.format,
                TemplateFormat::SkillPath | TemplateFormat::SkillShellPath
            ) {
                validate_skill_path(placeholder.name)?;
            }
        }
        rest = &rest[end + 1..];
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
        if let Some(placeholder) = template_placeholder(&rest[..end]) {
            if placeholder.format.is_argument_ref() {
                out.insert(placeholder.name.to_string());
            }
        }
        rest = &rest[end + 1..];
    }
}

fn render_template(
    template: &str,
    args: &Map<String, Value>,
    skill_root: Option<&Path>,
) -> Option<String> {
    let mut output = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        rest = &rest[start + 2..];
        let end = rest.find('}')?;
        let placeholder = &rest[..end];
        let placeholder = template_placeholder(placeholder)?;
        match placeholder.format {
            TemplateFormat::SkillPath | TemplateFormat::SkillShellPath => {
                validate_skill_path(placeholder.name).ok()?;
                let root = skill_root?;
                let path = root.join(placeholder.name).display().to_string();
                if placeholder.format == TemplateFormat::SkillShellPath {
                    output.push_str(&shell_quote_string(&path));
                } else {
                    output.push_str(&path);
                }
            }
            TemplateFormat::Json => {
                let value = args.get(placeholder.name)?;
                output.push_str(&serde_json::to_string(value).ok()?);
            }
            TemplateFormat::Shell => {
                let value = args.get(placeholder.name)?;
                output.push_str(&shell_quote_value(value)?);
            }
            TemplateFormat::ShellJoin => {
                let value = args.get(placeholder.name)?;
                output.push_str(&shell_quote_array(value)?);
            }
            TemplateFormat::Url => {
                let value = args.get(placeholder.name)?;
                output.push_str(&url_encode_value(value)?);
            }
            TemplateFormat::Raw => {
                let value = args.get(placeholder.name)?;
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
    Url,
    SkillPath,
    SkillShellPath,
}

impl TemplateFormat {
    fn is_argument_ref(self) -> bool {
        !matches!(self, Self::SkillPath | Self::SkillShellPath)
    }
}

struct TemplatePlaceholder<'a> {
    format: TemplateFormat,
    name: &'a str,
}

fn template_placeholder(placeholder: &str) -> Option<TemplatePlaceholder<'_>> {
    let trimmed = placeholder.trim();
    let (format, name, is_skill_path) = if let Some(name) = trimmed.strip_prefix("json:") {
        (TemplateFormat::Json, name.trim(), false)
    } else if let Some(name) = trimmed.strip_prefix("shell:") {
        (TemplateFormat::Shell, name.trim(), false)
    } else if let Some(name) = trimmed.strip_prefix("shell_join:") {
        (TemplateFormat::ShellJoin, name.trim(), false)
    } else if let Some(name) = trimmed.strip_prefix("url:") {
        (TemplateFormat::Url, name.trim(), false)
    } else if let Some(name) = trimmed.strip_prefix("skill_path:") {
        (TemplateFormat::SkillPath, name.trim(), true)
    } else if let Some(name) = trimmed.strip_prefix("skill_shell_path:") {
        (TemplateFormat::SkillShellPath, name.trim(), true)
    } else {
        (TemplateFormat::Raw, trimmed, false)
    };
    if is_skill_path {
        if name.is_empty() {
            return None;
        }
    } else if name.is_empty()
        || !name
            .chars()
            .all(|ch| ch == '_' || ch == '-' || ch.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(TemplatePlaceholder { format, name })
}

fn shell_quote_value(value: &Value) -> Option<String> {
    let text = if let Some(text) = value.as_str() {
        text.to_string()
    } else {
        serde_json::to_string(value).ok()?
    };
    Some(shell_quote_string(&text))
}

fn shell_quote_string(text: &str) -> String {
    format!("'{}'", text.replace('\'', r#"'"'"'"#))
}

fn shell_quote_array(value: &Value) -> Option<String> {
    let items = value.as_array()?;
    items
        .iter()
        .map(shell_quote_value)
        .collect::<Option<Vec<_>>>()
        .map(|quoted| quoted.join(" "))
}

fn url_encode_value(value: &Value) -> Option<String> {
    let text = if let Some(text) = value.as_str() {
        text.to_string()
    } else {
        serde_json::to_string(value).ok()?
    };
    Some(percent_encode(&text))
}

fn percent_encode(text: &str) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}
