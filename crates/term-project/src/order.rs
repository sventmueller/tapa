//! The order the Finder lists names in: numbers by their value, letters without regard to case or accents.

use std::cmp::Ordering;

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// A name prepared for sorting many times.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NameKey {
    base: Vec<char>,
    accented: Vec<char>,
    name: String,
}

impl NameKey {
    /// The key of `name`.
    pub(crate) fn new(name: &str) -> NameKey {
        let accented: Vec<char> = name.nfc().flat_map(char::to_lowercase).collect();
        let base = name
            .nfd()
            .filter(|c| !is_combining_mark(*c))
            .flat_map(char::to_lowercase)
            .collect();
        NameKey {
            base,
            accented,
            name: name.to_string(),
        }
    }
}

impl Ord for NameKey {
    fn cmp(&self, other: &NameKey) -> Ordering {
        natural(&self.base, &other.base)
            .then_with(|| natural(&self.accented, &other.accented))
            .then_with(|| {
                let a: Vec<char> = self.name.chars().collect();
                let b: Vec<char> = other.name.chars().collect();
                natural(&a, &b)
            })
            .then_with(|| self.name.cmp(&other.name))
    }
}

impl PartialOrd for NameKey {
    fn partial_cmp(&self, other: &NameKey) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Compares two names as the Finder sorts them.
#[cfg(test)]
pub(crate) fn compare(a: &str, b: &str) -> Ordering {
    NameKey::new(a).cmp(&NameKey::new(b))
}

fn natural(a: &[char], b: &[char]) -> Ordering {
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (start_a, start_b) = (i, j);
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let x = without_leading_zeros(&a[start_a..i]);
            let y = without_leading_zeros(&b[start_b..j]);
            let order = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
            if order != Ordering::Equal {
                return order;
            }
        } else {
            let order = (rank(a[i]), a[i]).cmp(&(rank(b[j]), b[j]));
            if order != Ordering::Equal {
                return order;
            }
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}

/// Punctuation and symbols before digits before letters, as in the Finder's collation.
fn rank(c: char) -> u8 {
    if c.is_ascii_digit() {
        1
    } else if c.is_alphanumeric() {
        2
    } else {
        0
    }
}

fn without_leading_zeros(digits: &[char]) -> &[char] {
    let zeros = digits.iter().take_while(|c| **c == '0').count();
    &digits[zeros..]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(names: &[&str]) -> Vec<String> {
        let mut names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        names.sort_by_cached_key(|n| NameKey::new(n));
        names
    }

    #[test]
    fn numbers_sort_by_their_value() {
        assert_eq!(
            sorted(&["file10.txt", "file2.txt", "file1.txt", "file02.txt"]),
            ["file1.txt", "file02.txt", "file2.txt", "file10.txt"]
        );
    }

    #[test]
    fn case_does_not_separate_names() {
        assert_eq!(
            sorted(&["banana", "Apple", "cherry", "Banana"]),
            ["Apple", "Banana", "banana", "cherry"]
        );
    }

    #[test]
    fn accented_letters_sort_beside_their_base_letter() {
        assert_eq!(
            sorted(&["Zebra", "Äpfel", "Apfel", "Birne"]),
            ["Apfel", "Äpfel", "Birne", "Zebra"]
        );
    }

    #[test]
    fn the_same_name_in_another_unicode_form_ties_only_on_its_bytes() {
        let composed = "\u{e4}";
        let decomposed = "a\u{308}";
        assert_ne!(compare(composed, decomposed), Ordering::Equal);
        assert_eq!(compare("a", composed), Ordering::Less);
        assert_eq!(compare(decomposed, "b"), Ordering::Less);
    }

    #[test]
    fn digits_come_before_letters_and_punctuation_before_both() {
        assert_eq!(sorted(&["b", "1", "_a", "a"]), ["_a", "1", "a", "b"]);
    }
}
