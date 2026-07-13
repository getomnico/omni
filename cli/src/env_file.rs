use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvLineKind {
    KeyValue { key: String, value: String },
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvLine {
    raw: String,
    kind: EnvLineKind,
    modified: bool,
}

#[derive(Debug, Clone, Default)]
pub struct EnvFile {
    lines: Vec<EnvLine>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvDiff {
    pub missing: Vec<String>,
    pub removed: Vec<String>,
    pub common: Vec<String>,
}

impl EnvFile {
    pub fn load(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read env file {}", path.display()))?;
        Ok(Self::parse(&content))
    }

    pub fn parse(content: &str) -> Self {
        let lines = content.lines().map(parse_line).collect();
        Self { lines }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        fs::write(path, self.to_string())
            .with_context(|| format!("failed to write env file {}", path.display()))
    }

    pub fn to_string(&self) -> String {
        let mut out = self
            .lines
            .iter()
            .map(|line| match &line.kind {
                EnvLineKind::KeyValue { key, value } if line.modified => format!("{key}={value}"),
                _ => line.raw.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        out.push('\n');
        out
    }

    pub fn keys(&self) -> BTreeSet<String> {
        self.lines
            .iter()
            .filter_map(|line| match &line.kind {
                EnvLineKind::KeyValue { key, .. } => Some(key.clone()),
                EnvLineKind::Other => None,
            })
            .collect()
    }

    pub fn raw_value(&self, key: &str) -> Option<String> {
        self.lines.iter().find_map(|line| match &line.kind {
            EnvLineKind::KeyValue { key: k, value } if k == key => Some(value.clone()),
            _ => None,
        })
    }

    pub fn value(&self, key: &str) -> Option<String> {
        self.raw_value(key).map(|value| normalize_value(&value))
    }

    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        let value = value.into();
        for line in &mut self.lines {
            if let EnvLineKind::KeyValue {
                key: existing,
                value: existing_value,
            } = &mut line.kind
            {
                if existing == key {
                    *existing_value = value;
                    line.modified = true;
                    return;
                }
            }
        }
        self.lines.push(EnvLine {
            raw: format!("{key}={value}"),
            kind: EnvLineKind::KeyValue {
                key: key.to_string(),
                value,
            },
            modified: true,
        });
    }

    pub fn append_section(&mut self, title: &str, values: &[(String, String)]) {
        if values.is_empty() {
            return;
        }
        if !self.lines.is_empty() {
            self.lines.push(EnvLine {
                raw: String::new(),
                kind: EnvLineKind::Other,
                modified: false,
            });
        }
        self.lines.push(EnvLine {
            raw: format!("# {title}"),
            kind: EnvLineKind::Other,
            modified: false,
        });
        for (key, value) in values {
            self.lines.push(EnvLine {
                raw: format!("{key}={value}"),
                kind: EnvLineKind::KeyValue {
                    key: key.clone(),
                    value: value.clone(),
                },
                modified: true,
            });
        }
    }

    pub fn diff_against_template(&self, template: &EnvFile) -> EnvDiff {
        let own = self.keys();
        let target = template.keys();
        EnvDiff {
            missing: target.difference(&own).cloned().collect(),
            removed: own.difference(&target).cloned().collect(),
            common: own.intersection(&target).cloned().collect(),
        }
    }
}

pub fn image_tag_from_release_tag(tag: &str) -> String {
    tag.trim_start_matches('v').to_string()
}

fn parse_line(raw: &str) -> EnvLine {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return EnvLine {
            raw: raw.to_string(),
            kind: EnvLineKind::Other,
            modified: false,
        };
    }

    let without_export = trimmed.strip_prefix("export ").unwrap_or(trimmed);
    let Some((key, value)) = without_export.split_once('=') else {
        return EnvLine {
            raw: raw.to_string(),
            kind: EnvLineKind::Other,
            modified: false,
        };
    };

    let key = key.trim();
    if key.is_empty()
        || !key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        || key.chars().next().is_some_and(|ch| ch.is_ascii_digit())
    {
        return EnvLine {
            raw: raw.to_string(),
            kind: EnvLineKind::Other,
            modified: false,
        };
    }

    EnvLine {
        raw: raw.to_string(),
        kind: EnvLineKind::KeyValue {
            key: key.to_string(),
            value: strip_inline_comment(value.trim()).to_string(),
        },
        modified: false,
    }
}

fn strip_inline_comment(value: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut prev_was_ws = false;
    for (idx, ch) in value.char_indices() {
        match ch {
            '\'' | '"' => {
                if quote == Some(ch) {
                    quote = None;
                } else if quote.is_none() {
                    quote = Some(ch);
                }
            }
            '#' if quote.is_none() && (idx == 0 || prev_was_ws) => return value[..idx].trim_end(),
            _ => {}
        }
        prev_was_ws = ch.is_whitespace();
    }
    value
}

fn normalize_value(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 {
        let first = value.as_bytes()[0] as char;
        let last = value.as_bytes()[value.len() - 1] as char;
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keys_and_values() {
        let env = EnvFile::parse("# hi\nA=1\nB=two # comment\nC=\"x#y\"\nexport D='z'\n");
        assert_eq!(env.value("A").as_deref(), Some("1"));
        assert_eq!(env.value("B").as_deref(), Some("two"));
        assert_eq!(env.value("C").as_deref(), Some("x#y"));
        assert_eq!(env.value("D").as_deref(), Some("z"));
    }

    #[test]
    fn set_updates_existing_key() {
        let mut env = EnvFile::parse("A=1\n");
        env.set("A", "2");
        env.set("B", "3");
        assert_eq!(env.to_string(), "A=2\nB=3\n");
    }

    #[test]
    fn preserves_unmodified_lines() {
        let mut env = EnvFile::parse("# title\nA=1 # inline docs\nB=2\n");
        env.set("B", "3");
        assert_eq!(env.to_string(), "# title\nA=1 # inline docs\nB=3\n");
    }

    #[test]
    fn diffs_against_template() {
        let local = EnvFile::parse("A=1\nOLD=x\n");
        let target = EnvFile::parse("A=1\nNEW=y\n");
        let diff = local.diff_against_template(&target);
        assert_eq!(diff.missing, vec!["NEW"]);
        assert_eq!(diff.removed, vec!["OLD"]);
        assert_eq!(diff.common, vec!["A"]);
    }

    #[test]
    fn image_tag_strips_leading_v() {
        assert_eq!(image_tag_from_release_tag("v1.2.3"), "1.2.3");
        assert_eq!(image_tag_from_release_tag("1.2.3"), "1.2.3");
    }
}
