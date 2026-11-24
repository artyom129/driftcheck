//! `driftcheck init`: a first mapping contract drafted from the two exports.
//!
//! Columns are paired by what they hold, not only by what they are called:
//! `full_name` and `name` pair because their values are the same people, and
//! `mrr` and `monthly_revenue` pair because `129.00` and `129` are the same
//! number. Header similarity only breaks ties. The draft is meant to be read
//! and adjusted, so every decision is reported back.

use crate::config::{Acceptance, Config, FieldMapping, Rule};
use crate::data::Dataset;
use crate::fuzzy::Pattern;
use crate::value::{norm_text, parse_date, parse_number};
use serde::Serialize;
use std::collections::HashSet;

/// Rows looked at per column. Enough to see the shape of the data, cheap on any size.
const SAMPLE: usize = 5000;
/// Share of non-empty values that must parse for a column to count as numbers, dates or emails.
const KIND_SHARE: f64 = 0.9;
/// Pairs below this score are not proposed.
const MIN_SCORE: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Empty,
    Text,
    Email,
    Number,
    Date,
}

impl Kind {
    fn rule(self) -> (Rule, f64) {
        match self {
            Kind::Email => (Rule::Email, 0.0),
            Kind::Number => (Rule::NumericTolerance, 0.01),
            Kind::Date => (Rule::Date, 0.0),
            Kind::Text | Kind::Empty => (Rule::Text, 0.0),
        }
    }

    fn compatible(self, other: Kind) -> bool {
        self == other
            || matches!((self, other), (Kind::Empty, _) | (_, Kind::Empty))
            || matches!((self, other), (Kind::Text, Kind::Email) | (Kind::Email, Kind::Text))
    }
}

#[derive(Debug, Clone)]
struct Profile {
    name: String,
    kind: Kind,
    /// Every distinct value of the column, in canonical form.
    values: HashSet<String>,
    /// Distinct canonical values of the sampled rows, used to probe the other side.
    sample: Vec<String>,
    /// Distinct values over non-empty values, 1.0 for a column with no repeats.
    uniqueness: f64,
    header: Pattern,
    header_tokens: Vec<String>,
}

fn looks_like_email(v: &str) -> bool {
    match v.trim().split_once('@') {
        Some((user, domain)) => !user.is_empty() && domain.contains('.') && !v.contains(' '),
        None => false,
    }
}

fn header_text(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn profile(data: &Dataset, col: usize) -> Profile {
    let n = data.len().min(SAMPLE);
    let cells: Vec<&str> = (0..n).filter_map(|r| data.cell(r, col)).filter(|v| !v.trim().is_empty()).collect();
    let share = |f: &dyn Fn(&str) -> bool| {
        if cells.is_empty() {
            0.0
        } else {
            cells.iter().filter(|v| f(v)).count() as f64 / cells.len() as f64
        }
    };
    let kind = if cells.is_empty() {
        Kind::Empty
    } else if share(&looks_like_email) >= KIND_SHARE {
        Kind::Email
    } else if share(&|v| parse_date(Some(v)).is_some()) >= KIND_SHARE {
        Kind::Date
    } else if share(&|v| parse_number(Some(v)).is_some()) >= KIND_SHARE {
        Kind::Number
    } else {
        Kind::Text
    };
    // Values in a form both sides can agree on, so 129.00 meets 1.290,00 / 10 and 02.03.2026 meets 2026-03-02.
    let canonical = |v: &str| -> String {
        match kind {
            Kind::Date => parse_date(Some(v)).map(|d| format!("d{d}")).unwrap_or_else(|| norm_text(Some(v))),
            Kind::Number => parse_number(Some(v)).map(|x| format!("n{x:.2}")).unwrap_or_else(|| norm_text(Some(v))),
            _ => norm_text(Some(v)),
        }
    };
    // The kind comes from the sample; the values come from every row, because
    // two shuffled exports share almost no keys between their first few thousand rows.
    let mut filled = 0usize;
    let mut values: HashSet<String> = HashSet::new();
    for v in (0..data.len()).filter_map(|r| data.cell(r, col)).filter(|v| !v.trim().is_empty()) {
        filled += 1;
        values.insert(canonical(v));
    }
    let uniqueness = if filled == 0 { 0.0 } else { values.len() as f64 / filled as f64 };
    let sample: Vec<String> = cells.iter().map(|v| canonical(v)).collect::<HashSet<_>>().into_iter().collect();
    let header = header_text(&data.headers[col]);
    Profile {
        name: data.headers[col].clone(),
        kind,
        uniqueness,
        header_tokens: header.split(' ').map(str::to_string).collect(),
        header: Pattern::new(&header),
        values,
        sample,
    }
}

/// Share of one side's sampled values that the other column holds anywhere,
/// taken in the direction where it is higher: a column of four plan names is
/// fully contained in a column that also has a fifth.
fn overlap(a: &Profile, b: &Profile) -> f64 {
    let contained = |probe: &[String], set: &HashSet<String>| {
        if probe.is_empty() {
            0.0
        } else {
            probe.iter().filter(|v| set.contains(*v)).count() as f64 / probe.len() as f64
        }
    };
    contained(&a.sample, &b.values).max(contained(&b.sample, &a.values))
}

fn header_similarity(a: &Profile, b: &Profile) -> f64 {
    let ratio = a.header.ratio(b.header.as_chars()) / 100.0;
    // `email` inside `email_address` is a strong hint even though the ratio is modest.
    let shared = a.header_tokens.iter().any(|t| t.len() > 2 && b.header_tokens.contains(t));
    if shared {
        ratio.max(0.75)
    } else {
        ratio
    }
}

const KEY_WORDS: [&str; 6] = ["id", "key", "code", "uuid", "number", "no"];

fn key_like(p: &Profile) -> bool {
    p.header_tokens.iter().any(|t| KEY_WORDS.contains(&t.as_str()))
}

/// One proposed pair, with the evidence behind it.
#[derive(Debug, Clone, Serialize)]
pub struct Pairing {
    pub source: String,
    pub target: String,
    pub kind: Kind,
    pub value_overlap: f64,
    pub header_similarity: f64,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Draft {
    pub config: Config,
    pub key: Pairing,
    pub pairs: Vec<Pairing>,
    pub unpaired_source: Vec<String>,
    pub unpaired_target: Vec<String>,
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

pub fn draft(source: &Dataset, target: &Dataset) -> Result<Draft, String> {
    if source.headers.is_empty() || target.headers.is_empty() {
        return Err("both files need a header row".into());
    }
    let sp: Vec<Profile> = (0..source.headers.len()).map(|c| profile(source, c)).collect();
    let tp: Vec<Profile> = (0..target.headers.len()).map(|c| profile(target, c)).collect();

    let mut candidates: Vec<(usize, usize, Pairing)> = Vec::new();
    for (i, a) in sp.iter().enumerate() {
        for (j, b) in tp.iter().enumerate() {
            if !a.kind.compatible(b.kind) {
                continue;
            }
            let values = overlap(a, b);
            let names = header_similarity(a, b);
            // Empty columns can only be paired by name.
            let score = if a.values.is_empty() || b.values.is_empty() {
                if names >= 0.9 { names * 0.6 } else { 0.0 }
            } else {
                0.7 * values + 0.3 * names
            };
            if score < MIN_SCORE {
                continue;
            }
            let kind = if a.kind == Kind::Empty { b.kind } else { a.kind };
            candidates.push((
                i,
                j,
                Pairing {
                    source: a.name.clone(),
                    target: b.name.clone(),
                    kind,
                    value_overlap: round3(values),
                    header_similarity: round3(names),
                    score: round3(score),
                },
            ));
        }
    }
    // Best evidence first; ties go to the earlier columns, so the draft is stable.
    candidates.sort_by(|x, y| y.2.score.total_cmp(&x.2.score).then(x.0.cmp(&y.0)).then(x.1.cmp(&y.1)));
    let mut used_s = vec![false; sp.len()];
    let mut used_t = vec![false; tp.len()];
    let mut chosen: Vec<(usize, usize, Pairing)> = Vec::new();
    for (i, j, p) in candidates {
        if !used_s[i] && !used_t[j] {
            used_s[i] = true;
            used_t[j] = true;
            chosen.push((i, j, p));
        }
    }
    chosen.sort_by_key(|(i, _, _)| *i);

    // The key: values (nearly) unique on both sides, preferably named like a key.
    let key_rank = |&(i, j, ref p): &(usize, usize, Pairing)| -> Option<(u8, f64)> {
        let (a, b) = (&sp[i], &tp[j]);
        if a.uniqueness < 0.98 || b.uniqueness < 0.98 {
            return None;
        }
        let named = u8::from(key_like(a) || key_like(b));
        let not_email = u8::from(p.kind != Kind::Email);
        Some((named * 2 + not_email, p.value_overlap))
    };
    let key_at = chosen
        .iter()
        .enumerate()
        .filter_map(|(n, c)| key_rank(c).map(|r| (n, r)))
        .max_by(|(_, a), (_, b)| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)))
        .map(|(n, _)| n)
        .ok_or("no column pair has unique values on both sides, so there is no key to match records by")?;
    let (_, _, key) = chosen.remove(key_at);

    let mappings: Vec<FieldMapping> = chosen
        .iter()
        .map(|(_, _, p)| {
            let (rule, tolerance) = p.kind.rule();
            FieldMapping { source: p.source.clone(), target: p.target.clone(), rule, tolerance }
        })
        .collect();
    // Renamed records are found by fields that tell people apart: emails and free text with few repeats.
    let fuzzy_fields: Vec<FieldMapping> = chosen
        .iter()
        .filter(|(i, _, p)| match p.kind {
            Kind::Email => true,
            // Names repeat across a large customer base, so a column with many
            // distinct values counts even when its share of repeats is high.
            Kind::Text => sp[*i].uniqueness >= 0.5 || sp[*i].values.len() >= 50,
            _ => false,
        })
        .map(|(_, _, p)| FieldMapping {
            source: p.source.clone(),
            target: p.target.clone(),
            rule: if p.kind == Kind::Email { Rule::Email } else { Rule::Text },
            tolerance: 0.0,
        })
        .collect();

    let config = Config {
        source_key: key.source.clone(),
        target_key: key.target.clone(),
        mappings,
        fuzzy_fields,
        fuzzy_threshold: 90.0,
        ambiguity_delta: 2.0,
        acceptance: Acceptance { min_quality_score: 99.0, ..Acceptance::default() },
    };
    let unpaired_source = (0..sp.len()).filter(|&i| !used_s[i]).map(|i| sp[i].name.clone()).collect();
    let unpaired_target = (0..tp.len()).filter(|&j| !used_t[j]).map(|j| tp[j].name.clone()).collect();
    Ok(Draft { config, key, pairs: chosen.into_iter().map(|(_, _, p)| p).collect(), unpaired_source, unpaired_target })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gen;
    use crate::reconcile::{reconcile, Status};

    fn generated(rows: usize) -> (Dataset, Dataset, gen::Generated) {
        let g = gen::build(rows, 42);
        let to = |h: &[&str], r: &[Vec<String>], name: &str| {
            Dataset::new(
                name,
                h.iter().map(|s| s.to_string()).collect(),
                r.iter().map(|row| row.iter().map(|c| Some(c.clone())).collect()).collect(),
            )
        };
        (to(&gen::SOURCE_HEADERS, &g.source, "legacy"), to(&gen::TARGET_HEADERS, &g.target, "migrated"), g)
    }

    #[test]
    fn pairs_columns_with_different_names_by_their_values() {
        let (s, t, _) = generated(1200);
        let d = draft(&s, &t).unwrap();
        assert_eq!((d.config.source_key.as_str(), d.config.target_key.as_str()), ("customer_id", "id"));
        let pairs: Vec<(&str, &str, Rule)> =
            d.config.mappings.iter().map(|m| (m.source.as_str(), m.target.as_str(), m.rule)).collect();
        assert_eq!(
            pairs,
            vec![
                ("full_name", "name", Rule::Text),
                ("email", "email_address", Rule::Email),
                ("plan", "tier", Rule::Text),
                ("mrr", "monthly_revenue", Rule::NumericTolerance),
                ("start_date", "activated_at", Rule::Date),
                ("country", "country_code", Rule::Text),
            ]
        );
        let fuzzy: Vec<&str> = d.config.fuzzy_fields.iter().map(|m| m.source.as_str()).collect();
        assert_eq!(fuzzy, vec!["full_name", "email"]);
        assert!(d.unpaired_source.is_empty() && d.unpaired_target.is_empty());
    }

    #[test]
    fn the_draft_finds_every_planted_defect() {
        let (s, t, g) = generated(3000);
        let d = draft(&s, &t).unwrap();
        let o = reconcile(&s, &t, &d.config).unwrap();
        let e = &g.expected;
        assert_eq!(o.count(Status::Matched), e.matched);
        assert_eq!(o.count(Status::FuzzyMatched), e.fuzzy_matched);
        assert_eq!(o.count(Status::Changed), e.changed);
        assert_eq!(o.count(Status::SourceOnly), e.source_only);
        assert_eq!(o.count(Status::TargetOnly), e.target_only);
    }

    #[test]
    fn keys_are_found_in_large_shuffled_exports() {
        // Far more rows than the sample: the first rows of each side barely share keys.
        let (s, t, _) = generated(30_000);
        let d = draft(&s, &t).unwrap();
        assert_eq!((d.config.source_key.as_str(), d.config.target_key.as_str()), ("customer_id", "id"));
        assert_eq!(d.config.mappings.len(), 6);
        let fuzzy: Vec<&str> = d.config.fuzzy_fields.iter().map(|m| m.source.as_str()).collect();
        assert_eq!(fuzzy, vec!["full_name", "email"]);
    }

    #[test]
    fn a_column_with_no_partner_is_reported() {
        let s = Dataset::new(
            "s",
            vec!["id".into(), "name".into(), "notes".into()],
            vec![
                vec![Some("1".into()), Some("Ava".into()), Some("call back".into())],
                vec![Some("2".into()), Some("Noah".into()), Some("vip".into())],
            ],
        );
        let t = Dataset::new(
            "t",
            vec!["user_id".into(), "display_name".into()],
            vec![vec![Some("1".into()), Some("Ava".into())], vec![Some("2".into()), Some("Noah".into())]],
        );
        let d = draft(&s, &t).unwrap();
        assert_eq!(d.config.source_key, "id");
        assert_eq!(d.config.target_key, "user_id");
        assert_eq!(d.unpaired_source, vec!["notes"]);
    }

    #[test]
    fn no_unique_column_means_no_key() {
        let rows = vec![vec![Some("a".into())], vec![Some("a".into())]];
        let s = Dataset::new("s", vec!["x".into()], rows.clone());
        let t = Dataset::new("t", vec!["x".into()], rows);
        assert!(draft(&s, &t).unwrap_err().contains("no key"));
    }

    #[test]
    fn kinds() {
        assert!(looks_like_email("ava@example.com"));
        assert!(!looks_like_email("ava at example"));
        assert_eq!(header_text("Email_Address"), "email address");
    }
}
