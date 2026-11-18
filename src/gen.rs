//! A reproducible CRM migration of any size, with a known number of every defect.
//!
//! The defect rates follow SyncProof's 1,200-customer test set and scale with
//! the row count. `expected.json` holds the truth, so a check can be held to it.

use serde::Serialize;
use std::path::Path;

const FIRST: &[&str] = &[
    "Ava", "Noah", "Mia", "Liam", "Sofia", "Oliver", "Emma", "Lucas", "Isla", "Ethan", "Zoe", "Mason", "Chloe",
    "Leo", "Nora", "Eli", "Ruby", "Owen", "Lena", "Hugo", "Maya", "Theo", "Iris", "Jack", "Alma", "Finn", "Dana",
    "Omar", "Vera", "Ivan", "Nina", "Adam", "Rosa", "Luka", "Elif", "Yuki", "Aiko", "Timur", "Aida", "Marek",
    "Petra", "Diego", "Lucia", "Mateo", "Camila", "Nikos", "Eleni", "Karim", "Leila", "Anton", "Greta",
];
const LAST: &[&str] = &[
    "Morgan", "Patel", "Chen", "Garcia", "Novak", "Smith", "Wilson", "Martin", "Kim", "Rossi", "Silva", "Haddad",
    "Tanaka", "Ivanov", "Kowalski", "Nguyen", "Fischer", "Dubois", "Costa", "Larsen", "Berg", "Ortiz", "Sato",
    "Khan", "Popescu", "Horvat", "Lindqvist", "Moreau", "Demir", "Aydin", "Suleiman", "Okafor", "Mendes",
    "Vargas", "Jensen", "Weber", "Keller", "Baumann", "Rizzo",
];
const PLANS: &[(&str, i64)] = &[("Starter", 49), ("Pro", 129), ("Business", 299), ("Enterprise", 1290)];
const COUNTRIES: &[&str] = &["US", "GB", "DE", "FR", "ES", "PL", "CA", "SG", "TR", "KZ", "AE", "BR"];
const DOMAINS: &[&str] = &["example.com", "example.org", "mail.example"];

/// SplitMix64: small, fast and the same on every platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, p: f64) -> bool {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64 <= p
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
    fn token(&mut self, len: usize) -> String {
        const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        (0..len).map(|_| ALPHABET[self.below(ALPHABET.len())] as char).collect()
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Expected {
    pub source_rows: usize,
    pub target_rows: usize,
    pub matched: usize,
    pub fuzzy_matched: usize,
    pub changed: usize,
    pub source_only: usize,
    pub target_only: usize,
    pub duplicate_source_keys: usize,
}

fn scaled(rows: usize, per_1200: usize) -> usize {
    ((rows as f64 * per_1200 as f64 / 1200.0).round() as usize).max(1)
}

fn money(amount: i64, style: usize) -> String {
    match style {
        0 => amount.to_string(),
        1 => format!("{amount}.00"),
        _ => {
            let digits = amount.to_string();
            let mut grouped = String::new();
            for (i, c) in digits.chars().enumerate() {
                if i > 0 && (digits.len() - i) % 3 == 0 {
                    grouped.push('.');
                }
                grouped.push(c);
            }
            format!("{grouped},00")
        }
    }
}

fn day(offset: i64) -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap() + chrono::Duration::days(offset)
}

pub struct Generated {
    pub source: Vec<Vec<String>>,
    pub target: Vec<Vec<String>>,
    pub expected: Expected,
}

pub const SOURCE_HEADERS: [&str; 7] = ["customer_id", "full_name", "email", "plan", "mrr", "start_date", "country"];
pub const TARGET_HEADERS: [&str; 7] =
    ["id", "name", "email_address", "tier", "monthly_revenue", "activated_at", "country_code"];

pub fn build(rows: usize, seed: u64) -> Generated {
    let mut rng = Rng(seed);
    let mut source: Vec<Vec<String>> = Vec::with_capacity(rows);
    for i in 0..rows {
        let name = format!("{} {}", rng.pick(FIRST), rng.pick(LAST));
        let (plan, price) = *rng.pick(PLANS);
        source.push(vec![
            format!("C-{}", 100_001 + i),
            name,
            format!("{}@{}", rng.token(12), rng.pick(DOMAINS)),
            plan.to_string(),
            format!("{price}.00"),
            day(rng.below(900) as i64).format("%Y-%m-%d").to_string(),
            rng.pick(COUNTRIES).to_string(),
        ]);
    }

    // Disjoint groups of rows, each with exactly one kind of damage.
    let mut order: Vec<usize> = (0..rows).collect();
    for i in (1..order.len()).rev() {
        let j = rng.below(i + 1);
        order.swap(i, j);
    }
    let mut take = |n: usize| -> Vec<usize> { (0..n.min(order.len())).filter_map(|_| order.pop()).collect() };
    let mark = |rows_: Vec<usize>, flags: &mut Vec<u8>, flag: u8| {
        for r in rows_ {
            flags[r] = flag;
        }
    };
    let mut kind = vec![0u8; rows];
    let (n_plan, n_price, n_date) = (scaled(rows, 30), scaled(rows, 20), scaled(rows, 15));
    let (n_missing, n_renamed, n_dup, n_extra) = (scaled(rows, 25), scaled(rows, 40), scaled(rows, 3), scaled(rows, 12));
    mark(take(n_plan), &mut kind, 1);
    mark(take(n_price), &mut kind, 2);
    mark(take(n_date), &mut kind, 3);
    mark(take(n_missing), &mut kind, 4);
    mark(take(n_renamed), &mut kind, 5);
    let duplicated = take(n_dup);

    let mut target: Vec<Vec<String>> = Vec::with_capacity(rows + n_extra);
    for (i, row) in source.iter().enumerate() {
        if kind[i] == 4 {
            continue;
        }
        let price: i64 = PLANS.iter().find(|p| p.0 == row[3]).unwrap().1;
        let mut out = vec![
            row[0].clone(),
            row[1].clone(),
            row[2].clone(),
            row[3].clone(),
            money(price, rng.below(3)),
            row[5].clone(),
            row[6].clone(),
        ];
        // Formatting noise that must not count as a difference.
        if rng.chance(0.30) {
            out[2] = out[2].to_uppercase();
        }
        if rng.chance(0.20) {
            out[1] = format!("  {} ", out[1]);
        }
        if rng.chance(0.35) {
            let d = chrono::NaiveDate::parse_from_str(&out[5], "%Y-%m-%d").unwrap();
            out[5] = d.format("%d.%m.%Y").to_string();
        }
        match kind[i] {
            1 => {
                let others: Vec<&str> = PLANS.iter().map(|p| p.0).filter(|p| *p != row[3]).collect();
                out[3] = rng.pick(&others).to_string();
            }
            2 => out[4] = money(price + [-20, -10, 10, 25][rng.below(4)], 1),
            3 => {
                let d = chrono::NaiveDate::parse_from_str(&row[5], "%Y-%m-%d").unwrap();
                out[5] = (d + chrono::Duration::days([-1, 1, 2][rng.below(3)])).format("%Y-%m-%d").to_string();
            }
            5 => out[0] = format!("CUS-{}", &row[0][2..]),
            _ => {}
        }
        target.push(out);
    }
    for j in 0..n_extra {
        target.push(vec![
            format!("C-{}", 900_001 + j),
            format!("{} {}", rng.pick(FIRST), rng.pick(LAST)),
            format!("{}@{}", rng.token(12), rng.pick(DOMAINS)),
            "Starter".into(),
            "49.00".into(),
            day(950 + j as i64).format("%Y-%m-%d").to_string(),
            rng.pick(COUNTRIES).to_string(),
        ]);
    }
    for &i in &duplicated {
        let copy = source[i].clone();
        source.push(copy);
    }
    for i in (1..target.len()).rev() {
        let j = rng.below(i + 1);
        target.swap(i, j);
    }

    let changed = n_plan + n_price + n_date;
    let source_only = n_missing + duplicated.len();
    let expected = Expected {
        source_rows: source.len(),
        target_rows: target.len(),
        matched: source.len() - changed - n_renamed - source_only,
        fuzzy_matched: n_renamed,
        changed,
        source_only,
        target_only: n_extra,
        duplicate_source_keys: duplicated.len(),
    };
    Generated { source, target, expected }
}

pub const MAPPING: &str = r#"{
  "source_key": "customer_id",
  "target_key": "id",
  "mappings": [
    {"source": "full_name", "target": "name", "rule": "text"},
    {"source": "email", "target": "email_address", "rule": "email"},
    {"source": "plan", "target": "tier", "rule": "exact"},
    {"source": "mrr", "target": "monthly_revenue", "rule": "numeric_tolerance", "tolerance": 0.01},
    {"source": "start_date", "target": "activated_at", "rule": "date", "tolerance": 0},
    {"source": "country", "target": "country_code", "rule": "exact"}
  ],
  "fuzzy_fields": [
    {"source": "full_name", "target": "name", "rule": "text"},
    {"source": "email", "target": "email_address", "rule": "email"}
  ],
  "fuzzy_threshold": 90,
  "ambiguity_delta": 2,
  "acceptance": {"max_changed": 0, "max_source_only": 0, "max_target_only": 0, "max_ambiguous": 0, "min_quality_score": 99}
}
"#;

fn write_rows(path: &Path, headers: &[&str], rows: &[Vec<String>]) -> Result<(), String> {
    let err = |e: csv::Error| format!("{}: {e}", path.display());
    let mut w = csv::Writer::from_path(path).map_err(err)?;
    w.write_record(headers).map_err(err)?;
    for r in rows {
        w.write_record(r).map_err(err)?;
    }
    w.flush().map_err(|e| format!("{}: {e}", path.display()))
}

pub fn write(dir: &Path, generated: &Generated) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    write_rows(&dir.join("legacy.csv"), &SOURCE_HEADERS, &generated.source)?;
    write_rows(&dir.join("migrated.csv"), &TARGET_HEADERS, &generated.target)?;
    std::fs::write(dir.join("mapping.json"), MAPPING).map_err(|e| e.to_string())?;
    let expected = serde_json::to_string_pretty(&generated.expected).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("expected.json"), expected + "\n").map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_data() {
        let a = build(500, 7);
        let b = build(500, 7);
        assert_eq!(a.source, b.source);
        assert_eq!(a.target, b.target);
        assert_eq!(a.expected, b.expected);
    }

    #[test]
    fn counts_add_up() {
        let g = build(1200, 42);
        let e = &g.expected;
        assert_eq!(e.changed, 65);
        assert_eq!(e.fuzzy_matched, 40);
        assert_eq!(e.source_only, 28);
        assert_eq!(e.target_only, 12);
        assert_eq!(e.matched + e.changed + e.fuzzy_matched + e.source_only, e.source_rows);
    }

    #[test]
    fn european_money() {
        assert_eq!(money(1290, 2), "1.290,00");
        assert_eq!(money(49, 2), "49,00");
        assert_eq!(money(129, 1), "129.00");
    }
}
