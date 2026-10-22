//! Applies byte-range edits to file contents. Pure, no I/O.

use crate::domain::model::Edit;

/// Apply edits to a file's contents. Fails if the text at an edit's span is not
/// what was planned (the file changed in between).
pub fn apply(src: &str, edits: &[&Edit]) -> Result<String, String> {
    let mut edits = edits.to_vec();
    edits.sort_by_key(|e| std::cmp::Reverse(e.span.start));
    let mut out = src.to_string();
    for e in edits {
        if out.get(e.span.clone()) != Some(e.expected.as_str()) {
            return Err(format!("{} changed since it was scanned", e.path.display()));
        }
        out.replace_range(e.span.clone(), &e.replacement);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_edits_back_to_front() {
        let edit = |span: std::ops::Range<usize>, expected: &str, replacement: &str| Edit {
            path: "ci.yml".into(),
            span,
            expected: expected.into(),
            replacement: replacement.into(),
        };
        let (a, b) = (edit(0..1, "a", "xx"), edit(2..3, "c", "yy"));
        assert_eq!(apply("abc", &[&a, &b]).unwrap(), "xxbyy");
    }

    #[test]
    fn rejects_stale_edits() {
        let edit = Edit {
            path: "ci.yml".into(),
            span: 0..2,
            expected: "v4".into(),
            replacement: "11bd71901bbe5b1630ceea73d27597364c9af683".into(),
        };
        assert!(apply("v5", &[&edit]).is_err());
    }
}
