use std::hash::Hasher;
use twox_hash::XxHash32;

const ALPHABET: &[u8; 16] = b"ZPMQVRWSNKTXJBYH";

fn normalize_line(line: &str) -> String {
    line.trim_end().replace('\r', "")
}

pub fn compute_hash(prev: &str, curr: &str, end: &str) -> u8 {
    let prev = normalize_line(prev);
    let curr = normalize_line(curr);
    let end = normalize_line(end);
    let input = format!("{}\0{}\0{}", prev, curr, end);
    let mut hasher = XxHash32::default();
    hasher.write(input.as_bytes());
    let hash = hasher.finish() as u32;
    (hash & 0xFF) as u8
}

pub fn hash_to_anchor(hash: u8) -> String {
    let idx0 = (hash >> 4) as usize;
    let idx1 = (hash & 0x0F) as usize;
    format!("{}{}", ALPHABET[idx0] as char, ALPHABET[idx1] as char)
}

pub fn compute_anchor(prev: &str, curr: &str, end: &str) -> String {
    hash_to_anchor(compute_hash(prev, curr, end))
}

pub fn format_line(line_num: usize, hash: &str, content: &str) -> String {
    format!("{:3}#{}:{}\n", line_num, hash, content)
}

pub fn format_line_trimmed(line_num: usize, hash: &str, content: &str) -> String {
    let trimmed = content.trim_end_matches('\n');
    format!("{:3}#{}:{}", line_num, hash, trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_trailing_whitespace() {
        assert_eq!(normalize_line("hello   "), "hello");
    }

    #[test]
    fn normalize_removes_carriage_return() {
        assert_eq!(normalize_line("hello\r\n"), "hello");
    }

    #[test]
    fn normalize_empty() {
        assert_eq!(normalize_line(""), "");
    }

    #[test]
    fn hash_deterministic() {
        let a = compute_hash("a", "b", "c");
        let b = compute_hash("a", "b", "c");
        assert_eq!(a, b);
    }

    #[test]
    fn hash_different_context_different_result() {
        let h1 = compute_hash("", "line", "");
        let h2 = compute_hash("prev", "line", "next");
        assert_ne!(
            h1, h2,
            "identical lines in different contexts should hash differently"
        );
    }

    #[test]
    fn hash_to_anchor_length_two() {
        let anchor = hash_to_anchor(0xAB);
        assert_eq!(anchor.len(), 2);
    }

    #[test]
    fn hash_to_anchor_alphabet_only() {
        for byte in 0..=255u8 {
            let anchor = hash_to_anchor(byte);
            for c in anchor.chars() {
                assert!(
                    ALPHABET.contains(&(c as u8)),
                    "char '{}' not in alphabet for byte {}",
                    c,
                    byte
                );
            }
        }
    }

    #[test]
    fn compute_anchor_roundtrip() {
        let anchor = compute_anchor("prev", "curr", "next");
        assert_eq!(anchor.len(), 2);
    }

    #[test]
    fn format_line_includes_line_hash_content() {
        let result = format_line(5, "KT", "hello");
        assert!(result.contains("5"));
        assert!(result.contains("KT"));
        assert!(result.contains("hello"));
        assert!(result.ends_with('\n'));
    }

    #[test]
    fn format_line_trimmed_no_newline() {
        let result = format_line_trimmed(3, "VR", "test");
        assert_eq!(result, "  3#VR:test");
    }

    #[test]
    fn compute_hash_all_empty() {
        let hash = compute_hash("", "", "");
        // Should not panic and produce a deterministic result
        let again = compute_hash("", "", "");
        assert_eq!(hash, again);
    }

    #[test]
    fn compute_hash_unicode() {
        let hash = compute_hash("日本語", "你好", "😊");
        let again = compute_hash("日本語", "你好", "😊");
        assert_eq!(hash, again);
    }
}
