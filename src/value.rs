//! How two cell values are compared. The rules follow SyncProof, so a contract
//! gives the same verdict in either tool.

use crate::config::{FieldMapping, Rule};
use chrono::{DateTime, NaiveDate};

/// Trimmed, lower-cased, inner whitespace collapsed to one space.
pub fn norm_text(value: Option<&str>) -> String {
    match value {
        None => String::new(),
        Some(v) => v.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase(),
    }
}

/// A key as it is stored in the data, so a patch plan can address the real row.
pub fn display_key(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|v| !v.is_empty()).map(str::to_string)
}

fn is_noise(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\'' | '$' | '€' | '£' | '¥' | '₽')
}

fn thousands_with_commas(text: &str) -> bool {
    // ^-?\d{1,3}(,\d{3})+$
    let body = text.strip_prefix('-').unwrap_or(text);
    let mut groups = body.split(',');
    let first = match groups.next() {
        Some(g) => g,
        None => return false,
    };
    if first.is_empty() || first.len() > 3 || !first.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let mut rest = 0;
    for g in groups {
        if g.len() != 3 || !g.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        rest += 1;
    }
    rest > 0
}

/// Parses 1234.5, 1,234.50, 1.234,50, 1 234,50 and $1,200 as numbers.
pub fn parse_number(value: Option<&str>) -> Option<f64> {
    let raw = value?;
    if raw.is_empty() {
        return None;
    }
    let mut text: String = raw.chars().filter(|c| !is_noise(*c)).collect();
    if text.contains(',') && text.contains('.') {
        if text.rfind(',') > text.rfind('.') {
            text = text.replace('.', "").replace(',', ".");
        } else {
            text = text.replace(',', "");
        }
    } else if text.contains(',') {
        text = if thousands_with_commas(&text) { text.replace(',', "") } else { text.replace(',', ".") };
    }
    text.parse::<f64>().ok()
}

const DATE_FORMATS: [&str; 4] = ["%Y-%m-%d", "%Y/%m/%d", "%m/%d/%Y", "%d.%m.%Y"];

/// Days since the epoch for ISO, YYYY/MM/DD, MM/DD/YYYY and DD.MM.YYYY dates.
pub fn parse_date(value: Option<&str>) -> Option<i64> {
    let text = value?.trim();
    if text.is_empty() {
        return None;
    }
    let head: String = text.chars().take(10).collect();
    for fmt in DATE_FORMATS {
        if let Ok(d) = NaiveDate::parse_from_str(&head, fmt) {
            return Some(d.signed_duration_since(NaiveDate::default()).num_days());
        }
    }
    let iso = text.replace('Z', "+00:00");
    DateTime::parse_from_rfc3339(&iso)
        .ok()
        .map(|dt| dt.date_naive().signed_duration_since(NaiveDate::default()).num_days())
}

fn is_empty(v: Option<&str>) -> bool {
    v.map_or(true, str::is_empty)
}

pub fn values_equal(left: Option<&str>, right: Option<&str>, mapping: &FieldMapping) -> bool {
    if is_empty(left) && is_empty(right) {
        return true;
    }
    let raw = |v: Option<&str>| v.unwrap_or("None").to_string();
    match mapping.rule {
        Rule::Exact => raw(left) == raw(right),
        Rule::Text | Rule::Email => norm_text(left) == norm_text(right),
        Rule::Numeric => parse_number(left) == parse_number(right),
        Rule::NumericTolerance => match (parse_number(left), parse_number(right)) {
            (Some(a), Some(b)) => (a - b).abs() <= mapping.tolerance,
            _ => false,
        },
        Rule::Date => match (parse_date(left), parse_date(right)) {
            (Some(a), Some(b)) => (a - b).abs() <= mapping.tolerance.trunc() as i64,
            _ => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(rule: Rule, tolerance: f64) -> FieldMapping {
        FieldMapping { source: "a".into(), target: "b".into(), rule, tolerance }
    }

    #[test]
    fn numbers_in_every_common_format() {
        assert_eq!(parse_number(Some("1234.5")), Some(1234.5));
        assert_eq!(parse_number(Some("1,234.50")), Some(1234.5));
        assert_eq!(parse_number(Some("1.234,50")), Some(1234.5));
        assert_eq!(parse_number(Some("1 234,50")), Some(1234.5));
        assert_eq!(parse_number(Some("$1,200")), Some(1200.0));
        assert_eq!(parse_number(Some("129,00")), Some(129.0));
        assert_eq!(parse_number(Some("-1,000,000")), Some(-1_000_000.0));
        assert_eq!(parse_number(Some("n/a")), None);
        assert_eq!(parse_number(None), None);
    }

    #[test]
    fn dates_in_every_common_format() {
        let iso = parse_date(Some("2026-03-02"));
        assert!(iso.is_some());
        assert_eq!(parse_date(Some("02.03.2026")), iso);
        assert_eq!(parse_date(Some("03/02/2026")), iso);
        assert_eq!(parse_date(Some("2026/03/02")), iso);
        assert_eq!(parse_date(Some("2026-03-02T10:15:00Z")), iso);
        assert_eq!(parse_date(Some("not a date")), None);
    }

    #[test]
    fn text_rules_ignore_case_and_padding() {
        assert!(values_equal(Some("  Ava  Morgan "), Some("ava morgan"), &m(Rule::Text, 0.0)));
        assert!(values_equal(Some("AVA@EXAMPLE.COM"), Some("ava@example.com"), &m(Rule::Email, 0.0)));
        assert!(!values_equal(Some("Pro"), Some("pro"), &m(Rule::Exact, 0.0)));
    }

    #[test]
    fn tolerances() {
        assert!(values_equal(Some("129.00"), Some("129.004"), &m(Rule::NumericTolerance, 0.01)));
        // A comma before exactly three digits is a thousands separator, as in SyncProof.
        assert!(!values_equal(Some("129.00"), Some("129,004"), &m(Rule::NumericTolerance, 0.01)));
        assert!(!values_equal(Some("129.00"), Some("128"), &m(Rule::NumericTolerance, 0.01)));
        assert!(values_equal(Some("2026-03-02"), Some("03.03.2026"), &m(Rule::Date, 1.0)));
        assert!(!values_equal(Some("2026-03-02"), Some("03.03.2026"), &m(Rule::Date, 0.0)));
    }

    #[test]
    fn two_empty_cells_agree_one_empty_does_not() {
        assert!(values_equal(None, Some(""), &m(Rule::Exact, 0.0)));
        assert!(!values_equal(None, Some("x"), &m(Rule::Text, 0.0)));
        assert!(!values_equal(Some("12"), None, &m(Rule::NumericTolerance, 1.0)));
    }

    #[test]
    fn display_key_keeps_case() {
        assert_eq!(display_key(Some("  C-1001 ")), Some("C-1001".into()));
        assert_eq!(display_key(Some("   ")), None);
    }
}
