//! driftcheck must reach the same verdict as SyncProof on SyncProof's own test set,
//! and the generator's truth must hold at a size where speed starts to matter.

use driftcheck::config::Config;
use driftcheck::data::Dataset;
use driftcheck::gen;
use driftcheck::reconcile::{reconcile, Status};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/syncproof-realistic").join(name)
}

#[test]
fn same_result_as_syncproof_on_its_1200_customers() {
    let source = Dataset::from_csv(&fixture("legacy_customers.csv")).unwrap();
    let target = Dataset::from_csv(&fixture("migrated_customers.csv")).unwrap();
    let config = Config::from_json(&std::fs::read_to_string(fixture("mapping.json")).unwrap()).unwrap();
    let expected: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fixture("expected.json")).unwrap()).unwrap();

    let o = reconcile(&source, &target, &config).unwrap();
    let want = |k: &str| expected[k].as_u64().unwrap() as usize;
    assert_eq!(o.summary.source_rows, want("source_rows"));
    assert_eq!(o.summary.target_rows, want("target_rows"));
    assert_eq!(o.count(Status::Matched), want("matched"));
    assert_eq!(o.count(Status::FuzzyMatched), want("fuzzy_matched"));
    assert_eq!(o.count(Status::Changed), want("changed"));
    assert_eq!(o.count(Status::SourceOnly), want("source_only"));
    assert_eq!(o.count(Status::TargetOnly), want("target_only"));
    assert_eq!(o.count(Status::Ambiguous), 0);
    assert_eq!(o.summary.duplicate_source_keys.len(), want("duplicate_source_keys"));
    // SyncProof reports 91.4 and these reasons for the same data.
    assert_eq!(o.summary.quality_score, 91.4);
    assert_eq!(
        o.summary.acceptance.reasons,
        vec![
            "65 changed records exceeds allowed maximum 0",
            "28 source-only records exceeds allowed maximum 0",
            "12 target-only records exceeds allowed maximum 0",
            "quality score 91.4 is below required 99.0",
        ]
    );
    // Formatting noise is not a difference: every change is a real one.
    assert_eq!(o.summary.changed_fields, want("changed"));
}

#[test]
fn generator_truth_holds_at_20000_rows() {
    let g = gen::build(20_000, 9);
    let headers = |h: &[&str]| h.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let rows = |r: &[Vec<String>]| -> Vec<Vec<Option<String>>> {
        r.iter().map(|row| row.iter().map(|c| Some(c.clone())).collect()).collect()
    };
    let source = Dataset::new("legacy", headers(&gen::SOURCE_HEADERS), rows(&g.source));
    let target = Dataset::new("migrated", headers(&gen::TARGET_HEADERS), rows(&g.target));
    let config = Config::from_json(gen::MAPPING).unwrap();

    let o = reconcile(&source, &target, &config).unwrap();
    let e = &g.expected;
    assert_eq!(o.count(Status::Matched), e.matched);
    assert_eq!(o.count(Status::FuzzyMatched), e.fuzzy_matched);
    assert_eq!(o.count(Status::Changed), e.changed);
    assert_eq!(o.count(Status::SourceOnly), e.source_only);
    assert_eq!(o.count(Status::TargetOnly), e.target_only);
    assert_eq!(o.count(Status::Ambiguous), 0);
}
