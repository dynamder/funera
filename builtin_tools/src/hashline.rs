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
