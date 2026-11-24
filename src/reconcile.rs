//! Two-pass reconciliation.
//!
//! Pass 1 claims exact key matches. Pass 2 looks for the remaining source rows
//! among target rows nobody owns yet, by name similarity. Because exact keys are
//! claimed first, a fuzzy guess can never take a row that belongs to another key,
//! and the result does not depend on row order. Two candidates that score within
//! `ambiguity_delta` of each other leave the row `ambiguous` for a person.

use crate::config::Config;
use crate::data::Dataset;
use crate::fuzzy::{record_score, Candidate, Probe};
use crate::value::{display_key, norm_text, values_equal};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Matched,
    FuzzyMatched,
    Changed,
    SourceOnly,
    TargetOnly,
    Ambiguous,
}

impl Status {
    pub const ALL: [Status; 6] = [
        Status::Matched,
        Status::FuzzyMatched,
        Status::Changed,
        Status::SourceOnly,
        Status::TargetOnly,
        Status::Ambiguous,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Matched => "matched",
            Status::FuzzyMatched => "fuzzy_matched",
            Status::Changed => "changed",
            Status::SourceOnly => "source_only",
            Status::TargetOnly => "target_only",
            Status::Ambiguous => "ambiguous",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Diff {
    pub source_field: String,
    pub target_field: String,
    pub source_value: Option<String>,
    pub target_value: Option<String>,
    pub rule: &'static str,
    pub tolerance: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Record {
    pub source_key: Option<String>,
    pub target_key: Option<String>,
    pub status: Status,
    pub match_score: f64,
    pub diffs: Vec<Diff>,
    #[serde(skip)]
    pub source_row: Option<usize>,
    #[serde(skip)]
    pub target_row: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Acceptance {
    pub passed: bool,
    pub reasons: Vec<String>,
}

/// How many matched records disagree on one mapped field.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FieldChange {
    pub source_field: String,
    pub target_field: String,
    pub rule: &'static str,
    pub changed: usize,
    /// Share of the compared records (matched, renamed or changed) that differ here.
    pub percent: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub source_rows: usize,
    pub target_rows: usize,
    pub results: usize,
    pub counts: BTreeMap<&'static str, usize>,
    pub changed_fields: usize,
    /// Every mapped field that differs at least once, the worst first.
    pub field_changes: Vec<FieldChange>,
    pub duplicate_target_keys: Vec<String>,
    pub duplicate_source_keys: Vec<String>,
    pub quality_score: f64,
    pub acceptance: Acceptance,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub summary: Summary,
    pub records: Vec<Record>,
}

impl Outcome {
    pub fn count(&self, status: Status) -> usize {
        self.summary.counts.get(status.as_str()).copied().unwrap_or(0)
    }
}

/// Floats printed the way Python prints them: 100.0, not 100.
pub fn py_float(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e16 {
        format!("{x:.1}")
    } else {
        format!("{x}")
    }
}

fn round_to(x: f64, places: i32) -> f64 {
    let f = 10f64.powi(places);
    (x * f).round() / f
}

#[derive(Default, Clone, Copy)]
struct Top2 {
    best: f64,
    best_index: usize,
    second: f64,
    seen: bool,
}

impl Top2 {
    fn push(mut self, score: f64, index: usize) -> Self {
        if !self.seen || score > self.best || (score == self.best && index < self.best_index) {
            if self.seen {
                self.second = self.second.max(self.best);
            }
            self.best = score;
            self.best_index = index;
            self.seen = true;
        } else {
            self.second = self.second.max(score);
        }
        self
    }

    fn merge(self, other: Top2) -> Self {
        if !other.seen {
            return self;
        }
        let merged = self.push(other.best, other.best_index);
        Top2 { second: merged.second.max(other.second), ..merged }
    }
}

pub fn reconcile(source: &Dataset, target: &Dataset, config: &Config) -> Result<Outcome, String> {
    let s_key = source.require(&[config.source_key.as_str()], "source")?[0];
    let t_key = target.require(&[config.target_key.as_str()], "target")?[0];
    let s_cols: Vec<&str> = config.mappings.iter().chain(&config.fuzzy_fields).map(|m| m.source.as_str()).collect();
    let t_cols: Vec<&str> = config.mappings.iter().chain(&config.fuzzy_fields).map(|m| m.target.as_str()).collect();
    source.require(&s_cols, "source")?;
    target.require(&t_cols, "target")?;
    let map_cols: Vec<(usize, usize)> = config
        .mappings
        .iter()
        .map(|m| (source.column(&m.source).unwrap(), target.column(&m.target).unwrap()))
        .collect();

    // Index targets by normalized key. Rows without a key are never matched by key.
    let mut target_index: HashMap<String, usize> = HashMap::with_capacity(target.len());
    let mut duplicate_target_keys: HashSet<String> = HashSet::new();
    for row in 0..target.len() {
        let key = norm_text(target.cell(row, t_key));
        if key.is_empty() {
            continue;
        }
        if target_index.contains_key(&key) {
            duplicate_target_keys.insert(key);
        } else {
            target_index.insert(key, row);
        }
    }

    let mut used = vec![false; target.len()];
    let mut assigned: Vec<Option<(usize, f64, Status)>> = vec![None; source.len()];
    let mut ambiguous: HashMap<usize, f64> = HashMap::new();
    let mut seen_source_keys: HashSet<String> = HashSet::with_capacity(source.len());
    let mut duplicate_source_keys: HashSet<String> = HashSet::new();

    // Pass 1: exact keys.
    for row in 0..source.len() {
        let key = norm_text(source.cell(row, s_key));
        if key.is_empty() {
            continue;
        }
        if !seen_source_keys.insert(key.clone()) {
            duplicate_source_keys.insert(key.clone());
        }
        if let Some(&t) = target_index.get(&key) {
            if !used[t] {
                used[t] = true;
                assigned[row] = Some((t, 100.0, Status::Matched));
            }
        }
    }

    // Pass 2: fuzzy matches for the rest, against targets nobody owns yet.
    if !config.fuzzy_fields.is_empty() {
        let fs: Vec<usize> = config.fuzzy_fields.iter().map(|m| source.column(&m.source).unwrap()).collect();
        let ft: Vec<usize> = config.fuzzy_fields.iter().map(|m| target.column(&m.target).unwrap()).collect();
        let mut free: Vec<usize> = (0..target.len()).filter(|&t| !used[t]).collect();
        // Only unclaimed targets are ever candidates, so only they are prepared.
        let mut candidates: Vec<Option<Vec<Candidate>>> = vec![None; target.len()];
        let prepared: Vec<(usize, Vec<Candidate>)> = free
            .par_iter()
            .map(|&t| (t, ft.iter().map(|&c| Candidate::new(&norm_text(target.cell(t, c)))).collect()))
            .collect();
        for (t, c) in prepared {
            candidates[t] = Some(c);
        }

        for row in 0..source.len() {
            if assigned[row].is_some() || free.is_empty() {
                continue;
            }
            let probe: Vec<Probe> = fs.iter().map(|&c| Probe::new(&norm_text(source.cell(row, c)))).collect();
            let top = free
                .par_iter()
                .fold(Top2::default, |acc, &t| {
                    acc.push(record_score(&probe, candidates[t].as_deref().unwrap_or(&[])), t)
                })
                .reduce(Top2::default, Top2::merge);
            if !top.seen || top.best < config.fuzzy_threshold {
                continue;
            }
            let second = if free.len() > 1 { top.second } else { -1.0 };
            if second >= config.fuzzy_threshold && (top.best - second).abs() <= config.ambiguity_delta {
                ambiguous.insert(row, top.best);
                continue;
            }
            used[top.best_index] = true;
            free.retain(|&t| t != top.best_index);
            assigned[row] = Some((top.best_index, top.best, Status::FuzzyMatched));
        }
    }

    // Build the records: one per source row, then every target nobody claimed.
    let mut records: Vec<Record> = (0..source.len())
        .into_par_iter()
        .map(|row| {
            let source_key = display_key(source.cell(row, s_key));
            if let Some(&score) = ambiguous.get(&row) {
                return Record {
                    source_key,
                    target_key: None,
                    status: Status::Ambiguous,
                    match_score: round_to(score, 2),
                    diffs: vec![],
                    source_row: Some(row),
                    target_row: None,
                };
            }
            match assigned[row] {
                None => Record {
                    source_key,
                    target_key: None,
                    status: Status::SourceOnly,
                    match_score: 0.0,
                    diffs: vec![],
                    source_row: Some(row),
                    target_row: None,
                },
                Some((t, score, status)) => {
                    let diffs: Vec<Diff> = config
                        .mappings
                        .iter()
                        .zip(&map_cols)
                        .filter_map(|(m, &(sc, tc))| {
                            let (l, r) = (source.cell(row, sc), target.cell(t, tc));
                            (!values_equal(l, r, m)).then(|| Diff {
                                source_field: m.source.clone(),
                                target_field: m.target.clone(),
                                source_value: l.map(str::to_string),
                                target_value: r.map(str::to_string),
                                rule: m.rule.as_str(),
                                tolerance: m.tolerance,
                            })
                        })
                        .collect();
                    Record {
                        source_key,
                        target_key: display_key(target.cell(t, t_key)),
                        status: if diffs.is_empty() { status } else { Status::Changed },
                        match_score: round_to(score, 2),
                        diffs,
                        source_row: Some(row),
                        target_row: Some(t),
                    }
                }
            }
        })
        .collect();
    for (t, claimed) in used.iter().enumerate() {
        if !claimed {
            records.push(Record {
                source_key: None,
                target_key: display_key(target.cell(t, t_key)),
                status: Status::TargetOnly,
                match_score: 0.0,
                diffs: vec![],
                source_row: None,
                target_row: Some(t),
            });
        }
    }

    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut changed_fields = 0;
    for r in &records {
        *counts.entry(r.status.as_str()).or_insert(0) += 1;
        changed_fields += r.diffs.len();
    }
    let get = |s: Status| counts.get(s.as_str()).copied().unwrap_or(0);

    let mut per_field = vec![0usize; config.mappings.len()];
    let field_index: HashMap<(&str, &str), usize> = config
        .mappings
        .iter()
        .enumerate()
        .map(|(i, m)| ((m.source.as_str(), m.target.as_str()), i))
        .collect();
    for d in records.iter().flat_map(|r| &r.diffs) {
        if let Some(&i) = field_index.get(&(d.source_field.as_str(), d.target_field.as_str())) {
            per_field[i] += 1;
        }
    }
    let compared = (get(Status::Matched) + get(Status::FuzzyMatched) + get(Status::Changed)).max(1) as f64;
    let mut field_changes: Vec<FieldChange> = config
        .mappings
        .iter()
        .zip(&per_field)
        .filter(|(_, &n)| n > 0)
        .map(|(m, &n)| FieldChange {
            source_field: m.source.clone(),
            target_field: m.target.clone(),
            rule: m.rule.as_str(),
            changed: n,
            percent: round_to(n as f64 / compared * 100.0, 2),
        })
        .collect();
    // Stable sort: equal counts keep the contract's order.
    field_changes.sort_by(|a, b| b.changed.cmp(&a.changed));

    let total = records.len().max(1) as f64;
    let clean = (get(Status::Matched) + get(Status::FuzzyMatched)) as f64;
    let quality_score = round_to(clean / total * 100.0, 1);

    let policy = &config.acceptance;
    let mut reasons = Vec::new();
    for (actual, maximum, label) in [
        (get(Status::Changed), policy.max_changed, "changed records"),
        (get(Status::SourceOnly), policy.max_source_only, "source-only records"),
        (get(Status::TargetOnly), policy.max_target_only, "target-only records"),
        (get(Status::Ambiguous), policy.max_ambiguous, "ambiguous matches"),
    ] {
        if actual > maximum {
            reasons.push(format!("{actual} {label} exceeds allowed maximum {maximum}"));
        }
    }
    if quality_score < policy.min_quality_score {
        reasons.push(format!(
            "quality score {} is below required {}",
            py_float(quality_score),
            py_float(policy.min_quality_score)
        ));
    }

    let mut dup_t: Vec<String> = duplicate_target_keys.into_iter().collect();
    let mut dup_s: Vec<String> = duplicate_source_keys.into_iter().collect();
    dup_t.sort();
    dup_s.sort();

    Ok(Outcome {
        summary: Summary {
            source_rows: source.len(),
            target_rows: target.len(),
            results: records.len(),
            counts,
            changed_fields,
            field_changes,
            duplicate_target_keys: dup_t,
            duplicate_source_keys: dup_s,
            quality_score,
            acceptance: Acceptance { passed: reasons.is_empty(), reasons },
        },
        records,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ds(name: &str, headers: &[&str], rows: &[&[&str]]) -> Dataset {
        Dataset::new(
            name,
            headers.iter().map(|h| h.to_string()).collect(),
            rows.iter()
                .map(|r| r.iter().map(|c| if c.is_empty() { None } else { Some(c.to_string()) }).collect())
                .collect(),
        )
    }

    fn config(json: &str) -> Config {
        Config::from_json(json).unwrap()
    }

    const CFG: &str = r#"{"source_key":"id","target_key":"id",
        "mappings":[{"source":"name","target":"name","rule":"text"},{"source":"mrr","target":"mrr","rule":"numeric_tolerance","tolerance":0.01}],
        "fuzzy_fields":[{"source":"name","target":"name"}]}"#;

    #[test]
    fn every_outcome() {
        let s = ds("s", &["id", "name", "mrr"], &[
            &["C-1", "Ava Morgan", "129.00"],
            &["C-2", "Noah Patel", "49"],
            &["C-3", "Mia Chen", "129"],
            &["C-4", "Liam Garcia", "299"],
        ]);
        let t = ds("t", &["id", "name", "mrr"], &[
            &["C-1", " AVA  MORGAN ", "129"],
            &["C-2", "Noah Patel", "39"],
            &["X-3", "Mia Chen", "129,00"],
            &["C-9", "Legacy Account", "49"],
        ]);
        let o = reconcile(&s, &t, &config(CFG)).unwrap();
        assert_eq!(o.count(Status::Matched), 1);
        assert_eq!(o.count(Status::Changed), 1);
        assert_eq!(o.count(Status::FuzzyMatched), 1);
        assert_eq!(o.count(Status::SourceOnly), 1);
        assert_eq!(o.count(Status::TargetOnly), 1);
        assert!(!o.summary.acceptance.passed);
        let changed = o.records.iter().find(|r| r.status == Status::Changed).unwrap();
        assert_eq!(changed.diffs[0].source_field, "mrr");
        // One of three compared records differs on mrr; name never differs.
        assert_eq!(o.summary.field_changes.len(), 1);
        assert_eq!(o.summary.field_changes[0].source_field, "mrr");
        assert_eq!(o.summary.field_changes[0].changed, 1);
        assert_eq!(o.summary.field_changes[0].percent, 33.33);
    }

    #[test]
    fn exact_keys_win_over_fuzzy_guesses() {
        // "Ava Morgan" under C-2 looks like the target C-1, but C-1 is owned by key.
        let s = ds("s", &["id", "name", "mrr"], &[&["C-2", "Ava Morgan", "1"], &["C-1", "Ava Morgan", "1"]]);
        let t = ds("t", &["id", "name", "mrr"], &[&["C-1", "Ava Morgan", "1"]]);
        let o = reconcile(&s, &t, &config(CFG)).unwrap();
        assert_eq!(o.records[1].status, Status::Matched);
        assert_eq!(o.records[0].status, Status::SourceOnly);
    }

    #[test]
    fn two_close_candidates_are_ambiguous() {
        // Both candidates score 94.7 against "jon smith", so neither can be chosen.
        let s = ds("s", &["id", "name", "mrr"], &[&["S-1", "Jon Smith", "1"]]);
        let t = ds("t", &["id", "name", "mrr"], &[&["T-1", "Jon Smith.", "1"], &["T-2", "Jon Smiths", "1"]]);
        let o = reconcile(&s, &t, &config(CFG)).unwrap();
        assert_eq!(o.count(Status::Ambiguous), 1);
        assert_eq!(o.count(Status::TargetOnly), 2);
    }

    #[test]
    fn empty_keys_never_match_and_repeats_are_reported() {
        let s = ds("s", &["id", "name", "mrr"], &[&["", "A", "1"], &["C-1", "B", "1"], &["c-1", "B", "1"]]);
        let t = ds("t", &["id", "name", "mrr"], &[&["", "Z", "1"], &["C-1", "B", "1"]]);
        let cfg = config(&CFG.replace(r#""fuzzy_fields":[{"source":"name","target":"name"}]"#, r#""fuzzy_fields":[]"#));
        let o = reconcile(&s, &t, &cfg).unwrap();
        assert_eq!(o.count(Status::Matched), 1);
        assert_eq!(o.count(Status::SourceOnly), 2);
        assert_eq!(o.count(Status::TargetOnly), 1);
        assert_eq!(o.summary.duplicate_source_keys, vec!["c-1".to_string()]);
    }

    #[test]
    fn a_missing_column_is_reported_up_front() {
        let s = ds("legacy", &["id", "name"], &[&["C-1", "A"]]);
        let t = ds("new", &["id", "name", "mrr"], &[&["C-1", "A", "1"]]);
        let err = reconcile(&s, &t, &config(CFG)).unwrap_err();
        assert!(err.contains("source dataset 'legacy': mrr"), "{err}");
    }

    #[test]
    fn messages_print_floats_like_python() {
        assert_eq!(py_float(100.0), "100.0");
        assert_eq!(py_float(91.4), "91.4");
    }
}
