use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Objective, language-agnostic signals computed from a file's patch.
/// These ground the model's per-file significance calls (and let us catch
/// a "critical" tag on a file whose additions contain no actual logic).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct FileMetrics {
    #[ts(type = "number")]
    pub additions: i64,
    #[ts(type = "number")]
    pub deletions: i64,
    /// Branch points introduced by added lines (if/loops/boolean ops) —
    /// a cyclomatic-complexity-delta proxy.
    #[ts(type = "number")]
    pub added_branches: i64,
    /// Function/class/type definitions introduced by added lines.
    #[ts(type = "number")]
    pub new_defs: i64,
    /// Share of added lines that are imports/includes, 0..1.
    pub import_share: f32,
    /// Deepest indentation level (~4 columns per level) among added lines.
    #[ts(type = "number")]
    pub max_nesting: i64,
}

impl FileMetrics {
    /// No added logic at all — pure wiring/renames/imports/data.
    pub fn is_logicless(&self) -> bool {
        self.added_branches == 0 && self.new_defs == 0
    }

    pub fn summary(&self) -> String {
        format!(
            "+{}/−{}, branches +{}, new defs {}, imports {:.0}%, nesting {}",
            self.additions,
            self.deletions,
            self.added_branches,
            self.new_defs,
            self.import_share * 100.0,
            self.max_nesting
        )
    }
}

/// Per-file metrics from a unified diff, in diff order.
pub fn diff_metrics(diff: &str) -> Vec<(String, FileMetrics)> {
    let mut out: Vec<(String, FileMetrics)> = Vec::new();
    let mut imports_added = 0i64;
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some((_, m)) = out.last_mut() {
                finalize(m, imports_added);
            }
            imports_added = 0;
            // "a/<old> b/<new>" — take the b-side path.
            let path = rest
                .rsplit_once(" b/")
                .map(|(_, b)| b.to_string())
                .unwrap_or_else(|| rest.to_string());
            out.push((path, FileMetrics::default()));
            continue;
        }
        let Some((_, m)) = out.last_mut() else { continue };
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if let Some(added) = line.strip_prefix('+') {
            m.additions += 1;
            let code = added.trim_start();
            if code.is_empty() || is_comment(code) {
                continue;
            }
            if is_import(code) {
                imports_added += 1;
                continue;
            }
            m.added_branches += branch_points(code);
            if is_def(code) {
                m.new_defs += 1;
            }
            m.max_nesting = m.max_nesting.max(indent_level(added));
        } else if line.starts_with('-') {
            m.deletions += 1;
        }
    }
    if let Some((_, m)) = out.last_mut() {
        finalize(m, imports_added);
    }
    out
}

fn finalize(m: &mut FileMetrics, imports_added: i64) {
    m.import_share = if m.additions > 0 {
        imports_added as f32 / m.additions as f32
    } else {
        0.0
    };
}

fn is_comment(code: &str) -> bool {
    code.starts_with("//")
        || code.starts_with('#') && !code.starts_with("#include") && !code.starts_with("#[")
        || code.starts_with('*')
        || code.starts_with("/*")
        || code.starts_with("--")
        || code.starts_with("\"\"\"")
        || code.starts_with("'''")
}

fn is_import(code: &str) -> bool {
    code.starts_with("import ")
        || (code.starts_with("from ") && code.contains(" import "))
        || code.starts_with("use ")
        || code.starts_with("using ")
        || code.starts_with("#include")
        || code.starts_with("require ")
        || code.contains("= require(")
        || code.starts_with("export ") && code.contains(" from ")
        || code.starts_with("} from ")
}

fn is_def(code: &str) -> bool {
    const DEF_PREFIXES: &[&str] = &[
        "def ",
        "async def ",
        "fn ",
        "pub fn ",
        "pub(crate) fn ",
        "async fn ",
        "pub async fn ",
        "function ",
        "async function ",
        "export function ",
        "export async function ",
        "export default function ",
        "class ",
        "export class ",
        "public class ",
        "interface ",
        "export interface ",
        "trait ",
        "pub trait ",
        "impl ",
        "struct ",
        "pub struct ",
        "enum ",
        "pub enum ",
        "func ",
        "type ",
        "export type ",
    ];
    DEF_PREFIXES.iter().any(|p| code.starts_with(p))
}

/// Count branch-introducing constructs on one added code line.
fn branch_points(code: &str) -> i64 {
    const KEYWORDS: &[&str] = &[
        "if", "elif", "else", "for", "while", "case", "when", "catch", "except", "match",
        "and", "or", "not",
    ];
    let mut n = 0i64;
    for word in code.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if KEYWORDS.contains(&word) {
            n += 1;
        }
    }
    n += code.matches("&&").count() as i64;
    n += code.matches("||").count() as i64;
    n += code.matches("?:").count() as i64;
    // One line rarely deserves more than a few points; cap regex-free noise.
    n.min(4)
}

fn indent_level(line: &str) -> i64 {
    let mut cols = 0i64;
    for c in line.chars() {
        match c {
            ' ' => cols += 1,
            '\t' => cols += 4,
            _ => break,
        }
    }
    cols / 4
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
diff --git a/core/graph.py b/core/graph.py
--- a/core/graph.py
+++ b/core/graph.py
@@ -1,3 +1,8 @@
+from core.nodes import (
+import os
+def is_long_call(state):
+    \"\"\"Docstring line with if and or words.\"\"\"
+    if duration > THRESHOLD and kind == \"Intake\":
+        return True
+    return False
+builder.add_edge(\"a\", \"b\")
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@ -1 +1,2 @@
+Just prose, no logic.
-old line
";

    #[test]
    fn computes_per_file_metrics() {
        let metrics = diff_metrics(SAMPLE);
        assert_eq!(metrics.len(), 2);
        let (path, m) = &metrics[0];
        assert_eq!(path, "core/graph.py");
        assert_eq!(m.additions, 8);
        assert_eq!(m.new_defs, 1, "def is_long_call counts");
        assert!(m.added_branches >= 2, "if + and count: {}", m.added_branches);
        assert!(m.import_share > 0.1 && m.import_share < 0.5);
        assert!(!m.is_logicless());

        let (path, m) = &metrics[1];
        assert_eq!(path, "README.md");
        assert_eq!(m.additions, 1);
        assert_eq!(m.deletions, 1);
        assert!(m.is_logicless());
    }

    #[test]
    fn docstrings_and_comments_do_not_count_as_branches() {
        let diff = "\
diff --git a/a.py b/a.py
+++ b/a.py
+# if this or that — a comment
+// while (true) in a comment
";
        let (_, m) = &diff_metrics(diff)[0];
        assert_eq!(m.added_branches, 0);
        assert!(m.is_logicless());
    }
}
