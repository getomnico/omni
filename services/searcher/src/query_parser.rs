use crate::operator_registry::OperatorRegistry;
use async_trait::async_trait;
use regex::Regex;
use serde_json::Value as JsonValue;
use shared::db::repositories::PersonRepository;
use shared::models::{AttributeFilter, DateFilter};
use shared::SourceType;
use std::collections::HashMap;
use time::OffsetDateTime;

#[async_trait]
pub trait PersonLookup: Send + Sync {
    async fn is_known_person(&self, term: &str) -> bool;
}

#[async_trait]
impl PersonLookup for PersonRepository {
    async fn is_known_person(&self, term: &str) -> bool {
        self.is_known_person(term).await.unwrap_or(false)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ParsedQuery {
    pub cleaned_query: String,
    pub attribute_filters: HashMap<String, AttributeFilter>,
    pub source_types: Vec<SourceType>,
    pub boosted_source_types: Vec<SourceType>,
    pub content_types: Vec<String>,
    pub date_filter: Option<DateFilter>,
    /// Strict author filter from explicit `by:` operator
    pub person_filters: Vec<String>,
    /// Soft person boost from natural language patterns ("emails from john", "docs by sarah")
    pub person_boosts: Vec<String>,
}

pub async fn parse(
    query: &str,
    person_lookup: &dyn PersonLookup,
    operator_registry: &OperatorRegistry,
) -> ParsedQuery {
    let mut result = ParsedQuery::default();
    let mut remaining = query.to_string();

    // Phase 1: Extract explicit operators
    remaining = extract_operators(&remaining, &mut result, operator_registry).await;

    // Phase 2: Extract natural language date patterns
    remaining = extract_natural_dates(&remaining, &mut result);

    // Phase 3: Extract natural language patterns (from/by/in)
    remaining = extract_natural_patterns(&remaining, &mut result, person_lookup).await;

    // Phase 4: Check if any word is a known source alias
    remaining = extract_source_word(&remaining, &mut result);

    // Clean up extra whitespace
    result.cleaned_query = remaining.split_whitespace().collect::<Vec<_>>().join(" ");

    result
}

async fn extract_operators(
    query: &str,
    result: &mut ParsedQuery,
    operator_registry: &OperatorRegistry,
) -> String {
    let re = match operator_registry.operator_regex().await {
        Some(re) => re,
        None => {
            // Fallback: universal operators only
            Regex::new(r#"(?i)\b(by|in|type|before|after):("([^"]+)"|(\S+))"#).unwrap()
        }
    };

    let mut remaining = query.to_string();

    let matches: Vec<_> = re
        .captures_iter(query)
        .map(|cap| {
            let full_match = cap.get(0).unwrap().as_str().to_string();
            let operator = cap[1].to_lowercase();
            let value = cap.get(3).or(cap.get(4)).unwrap().as_str().to_string();
            (full_match, operator, value)
        })
        .collect();

    for (full_match, operator, value) in matches {
        remaining = remaining.replacen(&full_match, "", 1);

        match operator.as_str() {
            // Universal operators — stay in the searcher
            "by" => {
                result.person_filters.push(value);
            }
            "in" => {
                if let Some(source) = resolve_source_alias(&value) {
                    result.source_types.push(source);
                }
            }
            "type" => {
                apply_type_filter(&value, &mut result.content_types);
            }
            "before" => {
                if let Some(dt) = parse_date_value(&value, true) {
                    result
                        .date_filter
                        .get_or_insert(DateFilter {
                            after: None,
                            before: None,
                        })
                        .before = Some(dt);
                }
            }
            "after" => {
                if let Some(dt) = parse_date_value(&value, false) {
                    result
                        .date_filter
                        .get_or_insert(DateFilter {
                            after: None,
                            before: None,
                        })
                        .after = Some(dt);
                }
            }
            // Dynamic operators — looked up from the registry
            _ => {
                if let Some(mapping) = operator_registry.get(&operator).await {
                    merge_attribute_filter(
                        &mut result.attribute_filters,
                        &mapping.attribute_key,
                        &value,
                    );
                }
            }
        }
    }

    remaining
}

fn extract_natural_dates(query: &str, result: &mut ParsedQuery) -> String {
    let mut remaining = query.to_string();
    let now = OffsetDateTime::now_utc();

    let patterns: &[(
        &str,
        Box<dyn Fn(OffsetDateTime) -> (Option<OffsetDateTime>, Option<OffsetDateTime>)>,
    )] = &[
        (
            r"(?i)\blast\s+week\b",
            Box::new(|now| {
                let after = now - time::Duration::days(7);
                (Some(after), None)
            }),
        ),
        (
            r"(?i)\blast\s+month\b",
            Box::new(|now| {
                let after = now - time::Duration::days(30);
                (Some(after), None)
            }),
        ),
        (
            r"(?i)\bthis\s+week\b",
            Box::new(|now| {
                let weekday = now.weekday().number_days_from_monday();
                let after = now - time::Duration::days(weekday as i64);
                let after = after.replace_time(time::Time::MIDNIGHT);
                (Some(after), None)
            }),
        ),
        (
            r"(?i)\byesterday\b",
            Box::new(|now| {
                let yesterday = now - time::Duration::days(1);
                let start = yesterday.replace_time(time::Time::MIDNIGHT);
                let end = start + time::Duration::days(1);
                (Some(start), Some(end))
            }),
        ),
        (
            r"(?i)\btoday\b",
            Box::new(|now| {
                let start = now.replace_time(time::Time::MIDNIGHT);
                (Some(start), None)
            }),
        ),
    ];

    for (pattern, compute) in patterns {
        let re = Regex::new(pattern).unwrap();
        if let Some(m) = re.find(&remaining) {
            let (after, before) = compute(now);
            let df = result.date_filter.get_or_insert(DateFilter {
                after: None,
                before: None,
            });
            if after.is_some() && df.after.is_none() {
                df.after = after;
            }
            if before.is_some() && df.before.is_none() {
                df.before = before;
            }
            remaining = format!("{}{}", &remaining[..m.start()], &remaining[m.end()..]);
        }
    }

    remaining
}

async fn extract_natural_patterns(
    query: &str,
    result: &mut ParsedQuery,
    person_lookup: &dyn PersonLookup,
) -> String {
    let mut remaining = query.to_string();

    // "from <word>" or "emails from <word>" — boost, not filter
    let from_re = Regex::new(r"(?i)\b(?:emails?\s+)?from\s+(\w+)\b").unwrap();
    if let Some(cap) = from_re.captures(&remaining) {
        let value = cap[1].to_string();
        if person_lookup.is_known_person(&value).await {
            result.person_boosts.push(value);
            remaining = remaining.replacen(cap.get(0).unwrap().as_str(), "", 1);
        }
    }

    // "by <word>" — boost, not filter
    let by_re = Regex::new(r"(?i)\b(?:docs?\s+)?by\s+(\w+)\b").unwrap();
    if let Some(cap) = by_re.captures(&remaining) {
        let value = cap[1].to_string();
        if person_lookup.is_known_person(&value).await {
            result.person_boosts.push(value);
            remaining = remaining.replacen(cap.get(0).unwrap().as_str(), "", 1);
        }
    }

    // "in <source>" — only match known source aliases
    let in_re = Regex::new(r"(?i)\bin\s+(\w+)\b").unwrap();
    if let Some(cap) = in_re.captures(&remaining) {
        let value = cap[1].to_string();
        if let Some(source) = resolve_source_alias(&value) {
            result.source_types.push(source);
            remaining = remaining.replacen(cap.get(0).unwrap().as_str(), "", 1);
        }
    }

    remaining
}

fn extract_source_word(query: &str, result: &mut ParsedQuery) -> String {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return trimmed.to_string();
    }
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    if words.len() < 2 {
        return trimmed.to_string();
    }
    for (i, word) in words.iter().enumerate() {
        if let Some(source) = resolve_source_alias(word) {
            result.boosted_source_types.push(source);
            let remaining: Vec<&str> = words
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, w)| *w)
                .collect();
            return remaining.join(" ");
        }
    }
    trimmed.to_string()
}

fn resolve_source_alias(alias: &str) -> Option<SourceType> {
    match alias.to_lowercase().as_str() {
        "drive" | "gdrive" | "google_drive" => Some(SourceType::GoogleDrive),
        "gmail" | "email" | "mail" => Some(SourceType::Gmail),
        "slack" => Some(SourceType::Slack),
        "confluence" | "wiki" => Some(SourceType::Confluence),
        "jira" => Some(SourceType::Jira),
        "github" | "gh" => Some(SourceType::Github),
        "notion" => Some(SourceType::Notion),
        "onedrive" | "one_drive" => Some(SourceType::OneDrive),
        "sharepoint" | "share_point" => Some(SourceType::SharePoint),
        "outlook" => Some(SourceType::Outlook),
        "hubspot" => Some(SourceType::Hubspot),
        "fireflies" => Some(SourceType::Fireflies),
        "clickup" | "click_up" => Some(SourceType::Clickup),
        "paperless" | "paperless_ngx" => Some(SourceType::PaperlessNgx),
        "web" | "website" => Some(SourceType::Web),
        _ => None,
    }
}

fn apply_type_filter(value: &str, content_types: &mut Vec<String>) {
    match value.to_lowercase().as_str() {
        "spreadsheet" | "sheet" => content_types.push("spreadsheet".to_string()),
        "doc" | "document" => content_types.push("document".to_string()),
        "slide" | "presentation" => content_types.push("presentation".to_string()),
        "pdf" => content_types.push("pdf".to_string()),
        "issue" => content_types.push("issue".to_string()),
        "pr" | "pull_request" => content_types.push("pull_request".to_string()),
        "page" => content_types.push("page".to_string()),
        "email" => {
            content_types.push("email_thread".to_string());
            content_types.push("email".to_string());
        }
        "meeting" | "transcript" => content_types.push("meeting_transcript".to_string()),
        _ => content_types.push(value.to_string()),
    }
}

fn merge_attribute_filter(filters: &mut HashMap<String, AttributeFilter>, key: &str, value: &str) {
    let json_val = JsonValue::String(value.to_string());
    match filters.get_mut(key) {
        Some(AttributeFilter::Exact(existing)) => {
            let existing_clone = existing.clone();
            *filters.get_mut(key).unwrap() = AttributeFilter::AnyOf(vec![existing_clone, json_val]);
        }
        Some(AttributeFilter::AnyOf(ref mut values)) => {
            values.push(json_val);
        }
        _ => {
            filters.insert(key.to_string(), AttributeFilter::Exact(json_val));
        }
    }
}

fn parse_date_value(value: &str, is_before: bool) -> Option<OffsetDateTime> {
    use time::format_description;

    // Full date: 2024-06-01
    if let Ok(date) = time::Date::parse(
        value,
        &format_description::parse("[year]-[month]-[day]").unwrap(),
    ) {
        let dt = date.with_time(if is_before {
            time::Time::from_hms(23, 59, 59).unwrap()
        } else {
            time::Time::MIDNIGHT
        });
        return Some(dt.assume_utc());
    }

    // Year-month: 2024-06
    if let Ok(date) = time::Date::parse(
        &format!("{}-01", value),
        &format_description::parse("[year]-[month]-[day]").unwrap(),
    ) {
        if is_before {
            // Last day of month
            let next_month = if date.month() == time::Month::December {
                time::Date::from_calendar_date(date.year() + 1, time::Month::January, 1).unwrap()
            } else {
                date.replace_month(date.month().next())
                    .unwrap()
                    .replace_day(1)
                    .unwrap()
            };
            let last_day = next_month - time::Duration::days(1);
            return Some(
                last_day
                    .with_time(time::Time::from_hms(23, 59, 59).unwrap())
                    .assume_utc(),
            );
        } else {
            return Some(date.with_time(time::Time::MIDNIGHT).assume_utc());
        }
    }

    // Year only: 2024
    if value.len() == 4 {
        if let Ok(year) = value.parse::<i32>() {
            if is_before {
                let date = time::Date::from_calendar_date(year, time::Month::December, 31).unwrap();
                return Some(
                    date.with_time(time::Time::from_hms(23, 59, 59).unwrap())
                        .assume_utc(),
                );
            } else {
                let date = time::Date::from_calendar_date(year, time::Month::January, 1).unwrap();
                return Some(date.with_time(time::Time::MIDNIGHT).assume_utc());
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operator_registry::OperatorRegistry;
    use shared::models::SearchOperator;

    use std::collections::HashSet;

    struct MockPersonLookup {
        known: HashSet<String>,
    }

    #[async_trait]
    impl PersonLookup for MockPersonLookup {
        async fn is_known_person(&self, term: &str) -> bool {
            self.known.contains(&term.to_lowercase())
        }
    }

    fn empty_lookup() -> MockPersonLookup {
        MockPersonLookup {
            known: HashSet::new(),
        }
    }

    fn lookup_with(names: &[&str]) -> MockPersonLookup {
        MockPersonLookup {
            known: names.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn default_registry() -> OperatorRegistry {
        OperatorRegistry::with_operators(vec![
            SearchOperator {
                operator: "from".to_string(),
                attribute_key: "sender".to_string(),
                value_type: "person".to_string(),
            },
            SearchOperator {
                operator: "channel".to_string(),
                attribute_key: "channel_name".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "status".to_string(),
                attribute_key: "status".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "label".to_string(),
                attribute_key: "labels".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "project".to_string(),
                attribute_key: "project_key".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "lang".to_string(),
                attribute_key: "language".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "assignee".to_string(),
                attribute_key: "assignee".to_string(),
                value_type: "person".to_string(),
            },
        ])
    }

    fn test_parse(query: &str) -> ParsedQuery {
        let lookup = empty_lookup();
        let registry = default_registry();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(parse(query, &lookup, &registry))
    }

    fn test_parse_with_lookup(query: &str, lookup: &dyn PersonLookup) -> ParsedQuery {
        let registry = default_registry();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(parse(query, lookup, &registry))
    }

    fn test_parse_with_registry(query: &str, registry: &OperatorRegistry) -> ParsedQuery {
        let lookup = empty_lookup();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(parse(query, &lookup, registry))
    }

    #[test]
    fn test_from_operator() {
        let parsed = test_parse("from:sarah@co.com report");
        assert_eq!(parsed.cleaned_query, "report");
        assert!(parsed.attribute_filters.contains_key("sender"));
        // Explicit from: is a strict attribute filter, not a person boost
        assert!(parsed.person_boosts.is_empty());
    }

    #[test]
    fn test_by_operator() {
        let parsed = test_parse("by:sarah docs");
        assert_eq!(parsed.cleaned_query, "docs");
        // Explicit by: is a strict person filter
        assert!(parsed.person_filters.contains(&"sarah".to_string()));
        assert!(parsed.person_boosts.is_empty());
    }

    #[test]
    fn test_in_operator() {
        let parsed = test_parse("in:drive meeting notes");
        assert_eq!(parsed.cleaned_query, "meeting notes");
        assert_eq!(parsed.source_types, vec![SourceType::GoogleDrive]);
    }

    #[test]
    fn test_in_operator_gmail_aliases() {
        for alias in &["gmail", "email", "mail"] {
            let parsed = test_parse(&format!("in:{} invoice", alias));
            assert_eq!(parsed.source_types, vec![SourceType::Gmail]);
        }
    }

    #[test]
    fn test_type_operator() {
        let parsed = test_parse("type:spreadsheet budget");
        assert_eq!(parsed.cleaned_query, "budget");
        assert_eq!(parsed.content_types, vec!["spreadsheet".to_string()]);
    }

    #[test]
    fn test_type_operator_pdf() {
        let parsed = test_parse("type:pdf invoice");
        assert_eq!(parsed.content_types, vec!["pdf".to_string()]);
    }

    #[test]
    fn test_type_operator_email() {
        let parsed = test_parse("type:email invoice");
        assert_eq!(
            parsed.content_types,
            vec!["email_thread".to_string(), "email".to_string()]
        );
    }

    #[test]
    fn test_type_operator_meeting() {
        let parsed = test_parse("type:meeting notes");
        assert_eq!(parsed.content_types, vec!["meeting_transcript".to_string()]);
    }

    #[test]
    fn test_channel_operator() {
        let parsed = test_parse("channel:eng standup");
        assert_eq!(parsed.cleaned_query, "standup");
        assert!(parsed.attribute_filters.contains_key("channel_name"));
    }

    #[test]
    fn test_status_operator() {
        let parsed = test_parse("status:done task");
        assert!(parsed.attribute_filters.contains_key("status"));
    }

    #[test]
    fn test_before_after_operators() {
        let parsed = test_parse("before:2024-06-01 after:2024-01-01 report");
        assert_eq!(parsed.cleaned_query, "report");
        let df = parsed.date_filter.unwrap();
        assert!(df.before.is_some());
        assert!(df.after.is_some());
    }

    #[test]
    fn test_before_year_only() {
        let parsed = test_parse("before:2024 report");
        let df = parsed.date_filter.unwrap();
        let before = df.before.unwrap();
        assert_eq!(before.year(), 2024);
        assert_eq!(before.month(), time::Month::December);
        assert_eq!(before.day(), 31);
    }

    #[test]
    fn test_after_year_only() {
        let parsed = test_parse("after:2024 report");
        let df = parsed.date_filter.unwrap();
        let after = df.after.unwrap();
        assert_eq!(after.year(), 2024);
        assert_eq!(after.month(), time::Month::January);
        assert_eq!(after.day(), 1);
    }

    #[test]
    fn test_quoted_values() {
        let parsed = test_parse(r#"from:"Sarah Jones" report"#);
        assert_eq!(parsed.cleaned_query, "report");
        assert!(parsed.attribute_filters.contains_key("sender"));
    }

    #[test]
    fn test_unknown_operator_passes_through() {
        let parsed = test_parse("error:connection timeout");
        assert_eq!(parsed.cleaned_query, "error:connection timeout");
        assert!(parsed.attribute_filters.is_empty());
    }

    #[test]
    fn test_multiple_labels_merge_to_anyof() {
        let parsed = test_parse("label:bug label:urgent");
        match parsed.attribute_filters.get("labels") {
            Some(AttributeFilter::AnyOf(values)) => {
                assert_eq!(values.len(), 2);
            }
            _ => panic!("Expected AnyOf filter for labels"),
        }
    }

    #[test]
    fn test_empty_query_after_extraction() {
        let parsed = test_parse("from:sarah");
        assert_eq!(parsed.cleaned_query, "");
        assert!(!parsed.attribute_filters.is_empty());
    }

    #[test]
    fn test_natural_language_from() {
        let lookup = lookup_with(&["john"]);
        let parsed = test_parse_with_lookup("emails from john", &lookup);
        assert_eq!(parsed.cleaned_query, "");
        assert!(parsed.person_boosts.contains(&"john".to_string()));
        assert!(parsed.attribute_filters.is_empty());
    }

    #[test]
    fn test_natural_language_from_unknown_person() {
        let parsed = test_parse("seashells from the seashore");
        assert_eq!(parsed.cleaned_query, "seashells from the seashore");
        assert!(parsed.person_boosts.is_empty());
    }

    #[test]
    fn test_natural_language_by() {
        let lookup = lookup_with(&["sarah"]);
        let parsed = test_parse_with_lookup("docs by sarah", &lookup);
        assert_eq!(parsed.cleaned_query, "");
        assert!(parsed.person_boosts.contains(&"sarah".to_string()));
        assert!(parsed.person_filters.is_empty());
    }

    #[test]
    fn test_natural_language_by_unknown_person() {
        let parsed = test_parse("seashells by the seashore");
        assert_eq!(parsed.cleaned_query, "seashells by the seashore");
        assert!(parsed.person_boosts.is_empty());
    }

    #[test]
    fn test_natural_language_in_source() {
        let parsed = test_parse("in slack standup");
        assert_eq!(parsed.cleaned_query, "standup");
        assert_eq!(parsed.source_types, vec![SourceType::Slack]);
    }

    #[test]
    fn test_natural_in_only_known_sources() {
        let parsed = test_parse("changes in production");
        assert_eq!(parsed.cleaned_query, "changes in production");
        assert!(parsed.source_types.is_empty());
    }

    #[test]
    fn test_source_word_extraction() {
        let parsed = test_parse("standup slack");
        assert_eq!(parsed.cleaned_query, "standup");
        assert!(parsed.source_types.is_empty());
        assert_eq!(parsed.boosted_source_types, vec![SourceType::Slack]);
    }

    #[test]
    fn test_source_word_alone_not_extracted() {
        let parsed = test_parse("slack");
        assert_eq!(parsed.cleaned_query, "slack");
        assert!(parsed.source_types.is_empty());
    }

    #[test]
    fn test_natural_date_last_week() {
        let parsed = test_parse("budget last week");
        assert_eq!(parsed.cleaned_query, "budget");
        assert!(parsed.date_filter.is_some());
        let df = parsed.date_filter.unwrap();
        assert!(df.after.is_some());
    }

    #[test]
    fn test_natural_date_last_month() {
        let parsed = test_parse("report last month");
        assert_eq!(parsed.cleaned_query, "report");
        assert!(parsed.date_filter.unwrap().after.is_some());
    }

    #[test]
    fn test_natural_date_yesterday() {
        let parsed = test_parse("standup yesterday");
        assert_eq!(parsed.cleaned_query, "standup");
        let df = parsed.date_filter.unwrap();
        assert!(df.after.is_some());
        assert!(df.before.is_some());
    }

    #[test]
    fn test_natural_date_today() {
        let parsed = test_parse("emails today");
        assert_eq!(parsed.cleaned_query, "emails");
        assert!(parsed.date_filter.unwrap().after.is_some());
    }

    #[test]
    fn test_combined_operators() {
        let parsed = test_parse("in:slack from:sarah status:done standup");
        assert_eq!(parsed.cleaned_query, "standup");
        assert_eq!(parsed.source_types, vec![SourceType::Slack]);
        assert!(parsed.attribute_filters.contains_key("sender"));
        assert!(parsed.attribute_filters.contains_key("status"));
    }

    #[test]
    fn test_source_alias_resolution() {
        let cases = vec![
            ("drive", SourceType::GoogleDrive),
            ("gdrive", SourceType::GoogleDrive),
            ("google_drive", SourceType::GoogleDrive),
            ("gmail", SourceType::Gmail),
            ("email", SourceType::Gmail),
            ("mail", SourceType::Gmail),
            ("slack", SourceType::Slack),
            ("confluence", SourceType::Confluence),
            ("wiki", SourceType::Confluence),
            ("jira", SourceType::Jira),
            ("github", SourceType::Github),
            ("gh", SourceType::Github),
            ("notion", SourceType::Notion),
            ("onedrive", SourceType::OneDrive),
            ("sharepoint", SourceType::SharePoint),
            ("outlook", SourceType::Outlook),
            ("hubspot", SourceType::Hubspot),
            ("fireflies", SourceType::Fireflies),
            ("clickup", SourceType::Clickup),
            ("click_up", SourceType::Clickup),
            ("paperless", SourceType::PaperlessNgx),
            ("paperless_ngx", SourceType::PaperlessNgx),
            ("web", SourceType::Web),
            ("website", SourceType::Web),
        ];

        for (alias, expected) in cases {
            assert_eq!(
                resolve_source_alias(alias),
                Some(expected),
                "Failed for alias: {}",
                alias
            );
        }
    }

    #[test]
    fn test_unknown_source_alias() {
        assert_eq!(resolve_source_alias("unknown"), None);
    }

    #[test]
    fn test_lang_operator() {
        let parsed = test_parse("lang:python error handling");
        assert_eq!(parsed.cleaned_query, "error handling");
        assert!(parsed.attribute_filters.contains_key("language"));
    }

    #[test]
    fn test_assignee_operator() {
        let parsed = test_parse("assignee:john bug fix");
        assert_eq!(parsed.cleaned_query, "bug fix");
        assert!(parsed.attribute_filters.contains_key("assignee"));
    }

    #[test]
    fn test_project_operator() {
        let parsed = test_parse("project:INFRA task");
        assert_eq!(parsed.cleaned_query, "task");
        assert!(parsed.attribute_filters.contains_key("project_key"));
    }

    #[test]
    fn test_colon_in_regular_text() {
        let parsed = test_parse("How to fix error: timeout");
        assert_eq!(parsed.cleaned_query, "How to fix error: timeout");
    }

    #[test]
    fn test_explicit_before_natural() {
        let lookup = lookup_with(&["john"]);
        let parsed = test_parse_with_lookup("from:sarah report from john", &lookup);
        // Explicit from:sarah → attribute filter (no boost)
        assert!(parsed.attribute_filters.contains_key("sender"));
        // Natural "from john" → boost (no attribute filter)
        assert!(parsed.person_boosts.contains(&"john".to_string()));
    }

    #[test]
    fn test_case_insensitivity() {
        let parsed = test_parse("FROM:sarah IN:Drive report");
        assert!(parsed.attribute_filters.contains_key("sender"));
        assert_eq!(parsed.source_types, vec![SourceType::GoogleDrive]);
    }

    #[test]
    fn test_type_aliases() {
        let cases = vec![
            ("sheet", "spreadsheet"),
            ("doc", "document"),
            ("slide", "presentation"),
            ("presentation", "presentation"),
            ("pdf", "pdf"),
            ("issue", "issue"),
            ("pr", "pull_request"),
            ("pull_request", "pull_request"),
            ("page", "page"),
        ];

        for (type_val, expected_content_type) in cases {
            let parsed = test_parse(&format!("type:{} query", type_val));
            assert!(
                parsed
                    .content_types
                    .contains(&expected_content_type.to_string()),
                "type:{} should produce content_type '{}'",
                type_val,
                expected_content_type
            );
        }
    }

    #[test]
    fn test_duplicate_operator_first_wins() {
        // When two connectors declare the same operator with different attribute_keys,
        // the registry deduplicates (first-wins), so only one attribute_key is used.
        let registry = OperatorRegistry::with_operators(vec![
            SearchOperator {
                operator: "status".to_string(),
                attribute_key: "status".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "status".to_string(),
                attribute_key: "state".to_string(),
                value_type: "text".to_string(),
            },
        ]);

        let parsed = test_parse_with_registry("status:done task", &registry);
        assert_eq!(parsed.cleaned_query, "task");
        // First-wins: "status" attribute_key is kept, "state" is ignored
        assert!(parsed.attribute_filters.contains_key("status"));
        assert!(!parsed.attribute_filters.contains_key("state"));
    }

    #[test]
    fn test_empty_registry_universal_operators_still_work() {
        let registry = OperatorRegistry::with_operators(vec![]);
        let parsed =
            test_parse_with_registry("in:slack type:pdf before:2024-01-01 report", &registry);
        assert_eq!(parsed.cleaned_query, "report");
        assert_eq!(parsed.source_types, vec![SourceType::Slack]);
        assert_eq!(parsed.content_types, vec!["pdf".to_string()]);
        assert!(parsed.date_filter.is_some());
    }
}
