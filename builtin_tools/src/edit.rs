use std::path::Path;

use async_trait::async_trait;
use funera_core::re_act::tool::{Tool, ToolCallError};
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};

use crate::hashline;

/// Operations supported by the edit tool.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditOp {
    /// Replace lines between `pos` and `end` anchors.
    Replace,
    /// Append lines after `pos` anchor.
    Append,
    /// Prepend lines before `pos` anchor.
    Prepend,
    /// Find and replace text by unique content match.
    ReplaceText,
}

/// A single edit operation on a file.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edit {
    #[serde(rename = "op")]
    pub operation: EditOp,
    #[serde(default)]
    pub pos: Option<String>,
    #[serde(default)]
    pub end: Option<String>,
    #[serde(default)]
    pub lines: Vec<String>,
    #[serde(default)]
    pub old_text: Option<String>,
    #[serde(default)]
    pub new_text: Option<String>,
}

/// Tool for editing files using hashline-anchored operations.
///
/// Works with anchors produced by the [`ReadTool`](crate::read::ReadTool).
/// Supports replace, append, prepend, and replace_text operations with
/// stale anchor detection and no-op loop prevention.
pub struct EditTool;

impl EditTool {
    fn parse_anchor(anchor: &str) -> Option<(usize, String)> {
        let anchor = anchor.trim();
        let parts: Vec<&str> = anchor.splitn(2, '#').collect();
        if parts.len() != 2 {
            return None;
        }
        let line_num: usize = parts[0].trim().parse().ok()?;
        let hash = parts[1].to_string();
        Some((line_num, hash))
    }

    fn verify_anchor(
        line_num: usize,
        expected_hash: &str,
        lines: &[String],
    ) -> Result<(), String> {
        let idx = line_num.saturating_sub(1);
        if idx >= lines.len() {
            return Err(format!(
                "[E_STALE_ANCHOR] line {} is beyond file length {}",
                line_num,
                lines.len()
            ));
        }
        let prev = if idx > 0 { &lines[idx - 1] } else { &String::new() };
        let curr = &lines[idx];
        let next = if idx + 1 < lines.len() {
            &lines[idx + 1]
        } else {
            &String::new()
        };
        let actual_hash = hashline::compute_anchor(prev, curr, next);
        if actual_hash != expected_hash {
            return Err(format!(
                "[E_STALE_ANCHOR] anchor mismatch at line {}: expected hash {}, actual hash {}",
                line_num, expected_hash, actual_hash
            ));
        }
        Ok(())
    }

    fn apply_ops(content: &str, edits: &[Edit]) -> Result<String, String> {
        let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();

        let mut ops: Vec<(usize, &Edit)> = Vec::new();
        for edit in edits {
            match edit.operation {
                EditOp::ReplaceText => {
                    let old_text = edit.old_text.as_deref().ok_or_else(|| {
                        "[E_INVALID_PATCH] replace_text requires oldText".to_string()
                    })?;
                    edit.new_text.as_deref().ok_or_else(|| {
                        "[E_INVALID_PATCH] replace_text requires newText".to_string()
                    })?;

                    let full = lines.join("\n");
                    let count = full.matches(old_text).count();
                    if count == 0 {
                        return Err(format!(
                            "[E_STALE_ANCHOR] replace_text: \"{}\" not found in file",
                            old_text
                        ));
                    }
                    if count > 1 {
                        return Err(format!(
                            "[E_INVALID_PATCH] replace_text: \"{}\" found {} times, not unique",
                            old_text, count
                        ));
                    }
                    ops.push((0, edit));
                }
                _ => {
                    let pos = edit.pos.as_deref().ok_or_else(|| {
                        "[E_INVALID_PATCH] edit requires pos anchor".to_string()
                    })?;
                    let (line_num, hash) = Self::parse_anchor(pos)
                        .ok_or_else(|| format!("[E_INVALID_PATCH] invalid anchor: {}", pos))?;
                    Self::verify_anchor(line_num, &hash, &lines)?;
                    if let Some(end_anchor) = &edit.end {
                        let (end_line, end_hash) = Self::parse_anchor(end_anchor)
                            .ok_or_else(|| {
                                format!("[E_INVALID_PATCH] invalid end anchor: {}", end_anchor)
                            })?;
                        Self::verify_anchor(end_line, &end_hash, &lines)?;
                        if end_line < line_num {
                            return Err(
                                "[E_INVALID_PATCH] end line before start line".to_string()
                            );
                        }
                    }
                    ops.push((line_num, edit));
                }
            }
        }

        ops.sort_by_key(|(line, _)| *line);
        ops.reverse();

        for (line_num, edit) in &ops {
            let idx = line_num.saturating_sub(1);
            match edit.operation {
                EditOp::Replace => {
                    let end_idx = if let Some(ref end_anchor) = edit.end {
                        let (end_line, _) = Self::parse_anchor(end_anchor).unwrap();
                        end_line.saturating_sub(1)
                    } else {
                        idx
                    };
                    if end_idx >= lines.len() || idx > end_idx {
                        return Err("[E_INVALID_PATCH] replace range out of bounds".to_string());
                    }
                    let _ = lines.splice(idx..=end_idx, edit.lines.clone());
                }
                EditOp::Append => {
                    let insert_at = if idx >= lines.len() - 1 {
                        lines.len()
                    } else {
                        idx + 1
                    };
                    let mut new_lines = edit.lines.clone();
                    new_lines.reverse();
                    for line in new_lines {
                        lines.insert(insert_at, line);
                    }
                }
                EditOp::Prepend => {
                    for (i, line) in edit.lines.iter().enumerate() {
                        lines.insert(idx + i, line.clone());
                    }
                }
                EditOp::ReplaceText => {
                    let old_text = edit.old_text.as_deref().unwrap();
                    let new_text = edit.new_text.as_deref().unwrap();
                    let full = lines.join("\n");
                    let new_full = full.replacen(old_text, new_text, 1);
                    lines = new_full.lines().map(|l| l.to_string()).collect();
                }
            }
        }

        let file_end = if content.ends_with('\n') { "\n" } else { "" };
        Ok(lines.join("\n") + file_end)
    }

    fn build_result_anchors(
        new_content: &str,
        edits: &[Edit],
    ) -> String {
        let new_lines: Vec<&str> = new_content.lines().collect();

        let mut affected_range = None;
        for edit in edits {
            if let Some(ref pos) = edit.pos {
                if let Some((line_num, _)) = Self::parse_anchor(pos) {
                    let end_line = edit
                        .end
                        .as_ref()
                        .and_then(|e| Self::parse_anchor(e))
                        .map(|(l, _)| l)
                        .unwrap_or(line_num);
                    let range = affected_range.get_or_insert((line_num, end_line));
                    range.0 = range.0.min(line_num);
                    range.1 = range.1.max(end_line);
                }
            }
        }

        let mut output = String::new();
        output.push_str("--- Anchors A-B ---\n");
        output.push_str("line#hash:content\n");

        let (start, end) = affected_range.unwrap_or((1, new_lines.len()));
        let context_start = start.saturating_sub(2);
        let context_end = (end + 1).min(new_lines.len());

        for i in context_start..context_end {
            if i >= new_lines.len() {
                break;
            }
            let prev = if i > 0 { new_lines[i - 1] } else { "" };
            let curr = new_lines[i];
            let next = if i + 1 < new_lines.len() {
                new_lines[i + 1]
            } else {
                ""
            };
            let anchor = hashline::compute_anchor(prev, curr, next);
            output.push_str(&hashline::format_line_trimmed(i + 1, &anchor, curr));
            output.push('\n');
        }

        output
    }
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Edit files using hashline-anchored operations: replace, append, prepend, replace_text."
    }

    fn schema(&self) -> JsonValue {
        json!({
            "type": "function",
            "function": {
                "name": "edit",
                "description": "Edit a file using hashline anchors from read output.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "filePath": {
                            "type": "string",
                            "description": "Absolute path to the file to edit"
                        },
                        "edits": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "op": {
                                        "type": "string",
                                        "enum": ["replace", "append", "prepend", "replace_text"]
                                    },
                                    "pos": {
                                        "type": "string",
                                        "description": "Hashline anchor (e.g. \"11#KT\")"
                                    },
                                    "end": {
                                        "type": "string",
                                        "description": "End anchor for range replace"
                                    },
                                    "lines": {
                                        "type": "array",
                                        "items": {"type": "string"},
                                        "description": "Lines to insert/replace with"
                                    },
                                    "oldText": {
                                        "type": "string",
                                        "description": "Text to find (for replace_text op)"
                                    },
                                    "newText": {
                                        "type": "string",
                                        "description": "Replacement text (for replace_text op)"
                                    }
                                },
                                "required": ["op"]
                            }
                        }
                    },
                    "required": ["filePath", "edits"]
                }
            }
        })
    }

    async fn execute(&self, args: JsonValue) -> Result<String, ToolCallError> {
        let file_path = args.get("filePath").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolCallError::ParameterMismatch(json!({"error": "missing filePath"}))
        })?;

        let edits: Vec<Edit> = serde_json::from_value(args.get("edits").cloned().unwrap_or_default())
            .map_err(|e| {
                ToolCallError::ParameterMismatch(json!({"error": format!("invalid edits: {}", e)}))
            })?;

        if edits.is_empty() {
            return Err(ToolCallError::ParameterMismatch(json!({"error": "no edits provided"})));
        }

        let path = Path::new(file_path);
        let content = tokio::fs::read_to_string(path).await.map_err(|e| {
            ToolCallError::ToolExecutionError(anyhow::anyhow!("cannot read file: {}", e))
        })?;

        for edit in &edits {
            for line in &edit.lines {
                if line.contains("#") {
                    let stripped = line.trim_start();
                    let parts: Vec<&str> = stripped.splitn(3, '#').collect();
                    if parts.len() >= 2 {
                        let after_hash = parts[1];
                        if after_hash.len() >= 2 {
                            let hash_chars: Vec<char> = after_hash.chars().collect();
                            if hash_chars[0].is_ascii_uppercase() && hash_chars[1].is_ascii_uppercase()
                            {
                                return Err(ToolCallError::ParameterMismatch(
                                    json!({"error": "[E_INVALID_PATCH] line contains LINE#HASH prefix"})
                                ));
                            }
                        }
                    }
                }
            }
        }

        let new_content = Self::apply_ops(&content, &edits).map_err(|e| {
            ToolCallError::ToolExecutionError(anyhow::anyhow!("{}", e))
        })?;

        let changed = new_content != content;
        if !changed {
            return Err(ToolCallError::ToolExecutionError(anyhow::anyhow!(
                "[E_NOOP_LOOP] edit produced no changes"
            )));
        }

        let temp_dir = path.parent().unwrap_or(Path::new("."));
        let temp_file = temp_dir.join(format!(
            ".{}.tmp",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));

        tokio::fs::write(&temp_file, &new_content).await.map_err(|e| {
            ToolCallError::ToolExecutionError(anyhow::anyhow!("cannot write temp file: {}", e))
        })?;

        tokio::fs::rename(&temp_file, path).await.map_err(|e| {
            let _ = tokio::fs::remove_file(&temp_file);
            ToolCallError::ToolExecutionError(anyhow::anyhow!("cannot rename temp file: {}", e))
        })?;

        let mut result = String::new();
        result.push_str(&format!(
            "Edited {} ({} bytes changed)\n",
            file_path,
            new_content.len()
        ));
        let anchors = Self::build_result_anchors(&new_content, &edits);
        result.push_str(&anchors);

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_edit(op: &str, pos: Option<&str>, lines: Vec<&str>, old_text: Option<&str>, new_text: Option<&str>) -> Edit {
        Edit {
            operation: serde_json::from_value(json!(op)).unwrap(),
            pos: pos.map(|s| s.to_string()),
            end: None,
            lines: lines.into_iter().map(String::from).collect(),
            old_text: old_text.map(String::from),
            new_text: new_text.map(String::from),
        }
    }

    fn test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("funera_edit_test_{}_{}", std::process::id(), label))
    }

    async fn write_test_file(label: &str, name: &str, content: &str) -> PathBuf {
        let dir = test_dir(label);
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join(name);
        tokio::fs::write(&path, content).await.unwrap();
        path
    }

    async fn cleanup(label: &str) {
        let _ = tokio::fs::remove_dir_all(test_dir(label)).await;
    }

    fn compute_line_anchor(lines: &[String], line_num: usize) -> String {
        let idx = line_num.saturating_sub(1);
        let prev = if idx > 0 { &lines[idx - 1] } else { &String::new() };
        let curr = &lines[idx];
        let next = if idx + 1 < lines.len() { &lines[idx + 1] } else { &String::new() };
        hashline::compute_anchor(prev, curr, next)
    }

    // --- parse_anchor tests ---

    #[test]
    fn parse_anchor_valid() {
        let (num, hash) = EditTool::parse_anchor("5#KT").unwrap();
        assert_eq!(num, 5);
        assert_eq!(hash, "KT");
    }

    #[test]
    fn parse_anchor_trimmed() {
        let (num, hash) = EditTool::parse_anchor("  10#VR  ").unwrap();
        assert_eq!(num, 10);
        assert_eq!(hash, "VR");
    }

    #[test]
    fn parse_anchor_no_hash() {
        assert!(EditTool::parse_anchor("5").is_none());
    }

    #[test]
    fn parse_anchor_non_numeric() {
        assert!(EditTool::parse_anchor("abc#KT").is_none());
    }

    // --- verify_anchor tests ---

    #[test]
    fn verify_anchor_matches() {
        let lines: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let hash = compute_line_anchor(&lines, 2);
        assert!(EditTool::verify_anchor(2, &hash, &lines).is_ok());
    }

    #[test]
    fn verify_anchor_mismatch() {
        let lines: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let result = EditTool::verify_anchor(2, "ZZ", &lines);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("STALE_ANCHOR"));
    }

    #[test]
    fn verify_anchor_out_of_bounds() {
        let lines: Vec<String> = vec!["a".into()];
        let result = EditTool::verify_anchor(5, "KT", &lines);
        assert!(result.is_err());
    }

    // --- apply_ops: replace ---

    #[test]
    fn replace_single_line() {
        let lines: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let hash = compute_line_anchor(&lines, 2);
        let edit = make_edit("replace", Some(&format!("2#{}", hash)), vec!["x", "y"], None, None);
        let result = EditTool::apply_ops("a\nb\nc", &[edit]).unwrap();
        assert_eq!(result, "a\nx\ny\nc");
    }

    // --- apply_ops: append ---

    #[test]
    fn append_after_line() {
        let lines: Vec<String> = vec!["a".into(), "b".into()];
        let hash = compute_line_anchor(&lines, 1);
        let edit = make_edit("append", Some(&format!("1#{}", hash)), vec!["x"], None, None);
        let result = EditTool::apply_ops("a\nb", &[edit]).unwrap();
        assert_eq!(result, "a\nx\nb");
    }

    #[test]
    fn append_at_end() {
        let lines: Vec<String> = vec!["a".into(), "b".into()];
        let hash = compute_line_anchor(&lines, 2);
        let edit = make_edit("append", Some(&format!("2#{}", hash)), vec!["c"], None, None);
        let result = EditTool::apply_ops("a\nb", &[edit]).unwrap();
        assert_eq!(result, "a\nb\nc");
    }

    // --- apply_ops: prepend ---

    #[test]
    fn prepend_before_line() {
        let lines: Vec<String> = vec!["a".into(), "b".into()];
        let hash = compute_line_anchor(&lines, 2);
        let edit = make_edit("prepend", Some(&format!("2#{}", hash)), vec!["x"], None, None);
        let result = EditTool::apply_ops("a\nb", &[edit]).unwrap();
        assert_eq!(result, "a\nx\nb");
    }

    // --- apply_ops: replace_text ---

    #[test]
    fn replace_text_unique() {
        let edit = make_edit("replace_text", None, vec![], Some("hello"), Some("hi"));
        let result = EditTool::apply_ops("say hello world", &[edit]).unwrap();
        assert_eq!(result, "say hi world");
    }

    #[test]
    fn replace_text_not_found() {
        let edit = make_edit("replace_text", None, vec![], Some("nope"), Some("hi"));
        let result = EditTool::apply_ops("hello world", &[edit]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("STALE_ANCHOR"));
    }

    #[test]
    fn replace_text_not_unique() {
        let edit = make_edit("replace_text", None, vec![], Some("a"), Some("b"));
        let result = EditTool::apply_ops("a a a", &[edit]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("INVALID_PATCH"));
    }

    // --- apply_ops: bottom-up ordering ---

    #[test]
    fn multiple_edits_bottom_up() {
        let lines: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let h1 = compute_line_anchor(&lines, 1);
        let h3 = compute_line_anchor(&lines, 3);
        let e1 = make_edit("replace", Some(&format!("1#{}", h1)), vec!["x"], None, None);
        let e2 = make_edit("replace", Some(&format!("3#{}", h3)), vec!["z"], None, None);
        let result = EditTool::apply_ops("a\nb\nc", &[e1, e2]).unwrap();
        assert_eq!(result, "x\nb\nz");
    }

    // --- apply_ops: stale anchor ---

    #[test]
    fn stale_anchor_rejected() {
        let edit = make_edit("replace", Some("2#ZZ"), vec!["x"], None, None);
        let result = EditTool::apply_ops("a\nb\nc", &[edit]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("STALE_ANCHOR"));
    }

    // --- Full execute flow ---

    #[tokio::test]
    async fn edit_execute_replace() {
        let path = write_test_file("replace", "exec_replace.txt", "line1\nline2\nline3\n").await;
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
        let hash = compute_line_anchor(&lines, 2);

        let tool = EditTool;
        let result = tool.execute(json!({
            "filePath": path.to_string_lossy(),
            "edits": [{
                "op": "replace",
                "pos": format!("2#{}", hash),
                "lines": ["modified!"]
            }]
        })).await;
        assert!(result.is_ok(), "replace failed: {:?}", result.err());
        let new_content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(new_content, "line1\nmodified!\nline3\n");
        cleanup("replace").await;
    }

    #[tokio::test]
    async fn edit_execute_replace_text() {
        let path = write_test_file("replacetext", "exec_replacetext.txt", "hello world\n").await;
        let tool = EditTool;
        let result = tool.execute(json!({
            "filePath": path.to_string_lossy(),
            "edits": [{
                "op": "replace_text",
                "oldText": "hello",
                "newText": "hi"
            }]
        })).await;
        assert!(result.is_ok(), "replace_text failed: {:?}", result.err());
        let new_content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(new_content, "hi world\n");
        cleanup("replacetext").await;
    }

    #[tokio::test]
    async fn edit_execute_missing_file() {
        let tool = EditTool;
        let result = tool.execute(json!({
            "filePath": "C:\\nonexistent_edit_test_file_xyz.txt",
            "edits": [{"op": "replace", "pos": "1#KT", "lines": ["x"]}]
        })).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn edit_execute_missing_file_path() {
        let tool = EditTool;
        let result = tool.execute(json!({"edits": []})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolCallError::ParameterMismatch(_) => {}
            e => panic!("expected ParameterMismatch, got {:?}", e),
        }
    }

    #[tokio::test]
    async fn edit_execute_noop_loop() {
        let path = write_test_file("noop", "noop.txt", "hello\n").await;
        let tool = EditTool;
        let result = tool.execute(json!({
            "filePath": path.to_string_lossy(),
            "edits": [{"op": "replace_text", "oldText": "hello", "newText": "hello"}]
        })).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("NOOP_LOOP"));
        cleanup("noop").await;
    }

    #[tokio::test]
    async fn edit_execute_append() {
        let path = write_test_file("append", "append.txt", "a\nb\n").await;
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
        let hash = compute_line_anchor(&lines, 2);

        let tool = EditTool;
        let result = tool.execute(json!({
            "filePath": path.to_string_lossy(),
            "edits": [{"op": "append", "pos": format!("2#{}", hash), "lines": ["c"]}]
        })).await;
        assert!(result.is_ok(), "append failed: {:?}", result.err());
        let new_content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(new_content, "a\nb\nc\n");
        cleanup("append").await;
    }

    #[test]
    fn empty_edits_returns_unchanged() {
        let result = EditTool::apply_ops("hello\nworld", &[]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "hello\nworld");
    }

    #[tokio::test]
    async fn edit_execute_prepend() {
        let path = write_test_file("prepend", "prepend.txt", "b\nc\n").await;
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
        let hash = compute_line_anchor(&lines, 1);

        let tool = EditTool;
        let result = tool.execute(json!({
            "filePath": path.to_string_lossy(),
            "edits": [{"op": "prepend", "pos": format!("1#{}", hash), "lines": ["a"]}]
        })).await;
        assert!(result.is_ok(), "prepend failed: {:?}", result.err());
        let new_content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(new_content, "a\nb\nc\n");
        cleanup("prepend").await;
    }
}
