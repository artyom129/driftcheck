//! The mapping contract. It reads the same JSON as SyncProof, so one contract
//! can be checked by either tool.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    #[default]
    Exact,
    Text,
    Email,
    Numeric,
    NumericTolerance,
    Date,
}

impl Rule {
    pub fn as_str(self) -> &'static str {
        match self {
            Rule::Exact => "exact",
            Rule::Text => "text",
            Rule::Email => "email",
            Rule::Numeric => "numeric",
            Rule::NumericTolerance => "numeric_tolerance",
            Rule::Date => "date",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FieldMapping {
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub rule: Rule,
    #[serde(default)]
    pub tolerance: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Acceptance {
    #[serde(default)]
    pub max_changed: usize,
    #[serde(default)]
    pub max_source_only: usize,
    #[serde(default)]
    pub max_target_only: usize,
    #[serde(default)]
    pub max_ambiguous: usize,
    #[serde(default = "default_min_quality")]
    pub min_quality_score: f64,
}

impl Default for Acceptance {
    fn default() -> Self {
        Self {
            max_changed: 0,
            max_source_only: 0,
            max_target_only: 0,
            max_ambiguous: 0,
            min_quality_score: default_min_quality(),
        }
    }
}

fn default_min_quality() -> f64 {
    100.0
}

fn default_threshold() -> f64 {
    90.0
}

fn default_delta() -> f64 {
    2.0
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub source_key: String,
    pub target_key: String,
    pub mappings: Vec<FieldMapping>,
    #[serde(default)]
    pub fuzzy_fields: Vec<FieldMapping>,
    #[serde(default = "default_threshold")]
    pub fuzzy_threshold: f64,
    #[serde(default = "default_delta")]
    pub ambiguity_delta: f64,
    #[serde(default)]
    pub acceptance: Acceptance,
}

impl Config {
    pub fn from_json(text: &str) -> Result<Self, String> {
        let config: Config = serde_json::from_str(text).map_err(|e| format!("mapping contract: {e}"))?;
        if !(0.0..=100.0).contains(&config.fuzzy_threshold) {
            return Err("mapping contract: fuzzy_threshold must be between 0 and 100".into());
        }
        if !(0.0..=25.0).contains(&config.ambiguity_delta) {
            return Err("mapping contract: ambiguity_delta must be between 0 and 25".into());
        }
        if !(0.0..=100.0).contains(&config.acceptance.min_quality_score) {
            return Err("mapping contract: min_quality_score must be between 0 and 100".into());
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_syncproof() {
        let c = Config::from_json(r#"{"source_key":"a","target_key":"b","mappings":[{"source":"x","target":"y"}]}"#).unwrap();
        assert_eq!(c.mappings[0].rule, Rule::Exact);
        assert_eq!(c.fuzzy_threshold, 90.0);
        assert_eq!(c.ambiguity_delta, 2.0);
        assert_eq!(c.acceptance.min_quality_score, 100.0);
    }

    #[test]
    fn rejects_out_of_range_threshold() {
        let err = Config::from_json(r#"{"source_key":"a","target_key":"b","mappings":[],"fuzzy_threshold":120}"#).unwrap_err();
        assert!(err.contains("fuzzy_threshold"));
    }
}
