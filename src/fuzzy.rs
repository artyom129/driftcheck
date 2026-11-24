//! Name similarity for records whose key changed.
//!
//! The score of two strings is the normalized Indel similarity,
//! 100 * 2 * LCS / (len a + len b), taken as the better of the plain strings and
//! the strings with their words sorted. The longest common subsequence is
//! computed with the bit-parallel algorithm of Allison and Dix (1986), one
//! machine word per 64 characters, so a pair costs a few operations per
//! character instead of a full dynamic-programming table.

use std::collections::HashMap;

/// Character masks of one string, built once and reused against every candidate.
#[derive(Debug, Clone)]
pub struct Pattern {
    chars: Vec<char>,
    ascii: Box<[u64; 128]>,
    other: HashMap<char, u64>,
}

impl Pattern {
    pub fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let mut ascii = Box::new([0u64; 128]);
        let mut other = HashMap::new();
        if chars.len() <= 64 {
            for (i, c) in chars.iter().enumerate() {
                let bit = 1u64 << i;
                if (*c as u32) < 128 {
                    ascii[*c as usize] |= bit;
                } else {
                    *other.entry(*c).or_insert(0) |= bit;
                }
            }
        }
        Self { chars, ascii, other }
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// The characters the pattern was built from.
    pub fn as_chars(&self) -> &[char] {
        &self.chars
    }

    #[inline]
    fn mask(&self, c: char) -> u64 {
        if (c as u32) < 128 {
            self.ascii[c as usize]
        } else {
            self.other.get(&c).copied().unwrap_or(0)
        }
    }

    /// Length of the longest common subsequence with `other`.
    pub fn lcs(&self, other: &[char]) -> usize {
        let m = self.chars.len();
        if m == 0 || other.is_empty() {
            return 0;
        }
        if m > 64 {
            return lcs_table(&self.chars, other);
        }
        let mut v: u64 = !0;
        for &c in other {
            let u = v & self.mask(c);
            v = v.wrapping_add(u) | (v & !self.mask(c));
        }
        let window = if m == 64 { !0u64 } else { (1u64 << m) - 1 };
        (!v & window).count_ones() as usize
    }

    /// Normalized Indel similarity with `other`, from 0 to 100.
    pub fn ratio(&self, other: &[char]) -> f64 {
        let total = self.len() + other.len();
        if total == 0 {
            return 100.0;
        }
        100.0 * 2.0 * self.lcs(other) as f64 / total as f64
    }
}

/// Plain dynamic programming, used only for strings longer than 64 characters.
fn lcs_table(a: &[char], b: &[char]) -> usize {
    let mut prev = vec![0usize; b.len() + 1];
    let mut cur = vec![0usize; b.len() + 1];
    for &ca in a {
        for (j, &cb) in b.iter().enumerate() {
            cur[j + 1] = if ca == cb { prev[j] + 1 } else { prev[j + 1].max(cur[j]) };
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

pub fn sorted_words(text: &str) -> String {
    let mut words: Vec<&str> = text.split_whitespace().collect();
    words.sort_unstable();
    words.join(" ")
}

/// A target value: just its characters, plain and word-sorted. Kept small,
/// because there is one per unclaimed target row.
#[derive(Debug, Clone)]
pub struct Candidate {
    plain: Vec<char>,
    sorted: Vec<char>,
}

impl Candidate {
    pub fn new(normalized: &str) -> Self {
        Self { plain: normalized.chars().collect(), sorted: sorted_words(normalized).chars().collect() }
    }

    pub fn is_empty(&self) -> bool {
        self.plain.is_empty()
    }
}

/// A source value with its character masks, built once per searched row.
#[derive(Debug, Clone)]
pub struct Probe {
    plain: Pattern,
    sorted: Pattern,
}

impl Probe {
    pub fn new(normalized: &str) -> Self {
        Self { plain: Pattern::new(normalized), sorted: Pattern::new(&sorted_words(normalized)) }
    }

    pub fn is_empty(&self) -> bool {
        self.plain.is_empty()
    }

    /// The better of the plain and the word-sorted similarity.
    pub fn score(&self, other: &Candidate) -> f64 {
        let plain = self.plain.ratio(&other.plain);
        if plain >= 100.0 {
            return plain;
        }
        plain.max(self.sorted.ratio(&other.sorted))
    }
}

/// Average similarity over the fuzzy fields, skipping fields empty on both sides.
pub fn record_score(source: &[Probe], target: &[Candidate]) -> f64 {
    let mut sum = 0.0;
    let mut n = 0usize;
    for (a, b) in source.iter().zip(target) {
        if a.is_empty() && b.is_empty() {
            continue;
        }
        sum += a.score(b);
        n += 1;
    }
    if n == 0 {
        0.0
    } else {
        sum / n as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcs(a: &str, b: &str) -> usize {
        Pattern::new(a).lcs(&b.chars().collect::<Vec<_>>())
    }

    #[test]
    fn bit_parallel_matches_the_table() {
        let pairs = [
            ("ava morgan", "ava morgen"),
            ("kitten", "sitting"),
            ("", "abc"),
            ("abc", ""),
            ("same", "same"),
            ("ёлка зелёная", "зелёная ёлка"),
            ("northstar components ltd", "north star components"),
        ];
        for (a, b) in pairs {
            let table = lcs_table(&a.chars().collect::<Vec<_>>(), &b.chars().collect::<Vec<_>>());
            assert_eq!(lcs(a, b), table, "{a} / {b}");
        }
    }

    #[test]
    fn long_strings_fall_back_to_the_table() {
        let a = "x".repeat(70) + "abc";
        let b = "x".repeat(65) + "abd";
        assert_eq!(lcs(&a, &b), 67);
    }

    #[test]
    fn sixty_four_characters_use_the_whole_word() {
        let a = "a".repeat(64);
        assert_eq!(lcs(&a, &a), 64);
    }

    #[test]
    fn ratio_values() {
        let p = Pattern::new("kitten");
        let r = p.ratio(&"sitting".chars().collect::<Vec<_>>());
        assert!((r - 61.538).abs() < 0.01, "{r}");
        assert_eq!(Pattern::new("abc").ratio(&"abc".chars().collect::<Vec<_>>()), 100.0);
    }

    #[test]
    fn word_order_does_not_matter() {
        assert_eq!(Probe::new("morgan ava").score(&Candidate::new("ava morgan")), 100.0);
    }

    #[test]
    fn empty_fields_are_skipped() {
        let s = vec![Probe::new("ava morgan"), Probe::new("")];
        let t = vec![Candidate::new("ava morgan"), Candidate::new("")];
        assert_eq!(record_score(&s, &t), 100.0);
    }
}
