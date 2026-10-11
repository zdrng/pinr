//! Shared tree-sitter helpers for the YAML extractors.

use std::ops::Range;

use tree_sitter::Node;

#[cfg(test)]
use crate::domain::model::{Found, Kind};

/// A scalar value in a YAML document.
#[derive(Debug, Clone, PartialEq)]
pub struct Scalar<'a> {
    /// Keys leading to the value, e.g. `["jobs", "build", "steps", "[]", "uses"]`.
    /// Sequence items are `"[]"`.
    pub path: Vec<&'a str>,
    /// Start byte of the innermost mapping containing the value. Values of the
    /// same mapping share it, e.g. `project` and `ref` of one GitLab include.
    pub mapping: usize,
    /// The value without quotes. Escapes are not processed.
    pub span: Range<usize>,
    pub text: &'a str,
}

/// Every scalar value (not keys) in `src`, in document order. Block scalars
/// (`|`, `>`) and aliases are skipped.
pub fn scalars(src: &str) -> Vec<Scalar<'_>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_yaml::LANGUAGE.into())
        .expect("tree-sitter-yaml is compatible");
    let Some(tree) = parser.parse(src, None) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    walk(tree.root_node(), src, &mut Vec::new(), 0, &mut out);
    out
}

fn walk<'a>(
    node: Node,
    src: &'a str,
    path: &mut Vec<&'a str>,
    mapping: usize,
    out: &mut Vec<Scalar<'a>>,
) {
    match node.kind() {
        "plain_scalar" | "single_quote_scalar" | "double_quote_scalar" => {
            let span = unquoted(node);
            out.push(Scalar {
                path: path.clone(),
                mapping,
                text: &src[span.clone()],
                span,
            });
        }
        "block_mapping" | "flow_mapping" => {
            for child in node.named_children(&mut node.walk()) {
                walk(child, src, path, node.start_byte(), out);
            }
        }
        "block_mapping_pair" | "flow_pair" => {
            let key = node.child_by_field_name("key").and_then(scalar_span);
            if let (Some(key), Some(value)) = (key, node.child_by_field_name("value")) {
                path.push(&src[key]);
                walk(value, src, path, mapping, out);
                path.pop();
            }
        }
        "block_sequence_item" => {
            path.push("[]");
            for child in node.named_children(&mut node.walk()) {
                walk(child, src, path, mapping, out);
            }
            path.pop();
        }
        "flow_sequence" => {
            for child in node.named_children(&mut node.walk()) {
                path.push("[]");
                walk(child, src, path, mapping, out);
                path.pop();
            }
        }
        // Documents, nodes wrapping anchors/tags, block sequences, …
        _ => {
            for child in node.named_children(&mut node.walk()) {
                walk(child, src, path, mapping, out);
            }
        }
    }
}

/// The span of the scalar in a key node (which may also hold an anchor or tag).
fn scalar_span(node: Node) -> Option<Range<usize>> {
    match node.kind() {
        "plain_scalar" | "single_quote_scalar" | "double_quote_scalar" => Some(unquoted(node)),
        "flow_node" | "block_node" => node.named_children(&mut node.walk()).find_map(scalar_span),
        _ => None,
    }
}

fn unquoted(node: Node) -> Range<usize> {
    let r = node.byte_range();
    match node.kind() {
        "plain_scalar" => r,
        _ => r.start + 1..(r.end - 1).max(r.start + 1),
    }
}

/// Test helper: each found as (text at its span, reference or `"warning"`).
#[cfg(test)]
pub fn summarize(src: &str, found: Vec<Found>) -> Vec<(&str, String)> {
    found
        .into_iter()
        .map(|f| {
            let what = match f.kind {
                Kind::Ref(r) => r.to_string(),
                Kind::Warning(_) => "warning".to_string(),
            };
            (&src[f.span], what)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(src: &str) -> Vec<(String, &str)> {
        scalars(src)
            .into_iter()
            .map(|s| (s.path.join("."), s.text))
            .collect()
    }

    #[test]
    fn block_style_with_quotes() {
        let src = "jobs:\n  build:\n    steps:\n      - uses: 'a@v1'\n      - run: \"echo\"\n";
        assert_eq!(
            paths(src),
            [
                ("jobs.build.steps.[].uses".to_string(), "a@v1"),
                ("jobs.build.steps.[].run".to_string(), "echo"),
            ]
        );
    }

    #[test]
    fn flow_style_and_quoted_keys() {
        let src = "\"services\": {web: {image: 'nginx'}}\nlist: [a, \"b\", {k: c}]\n";
        assert_eq!(
            paths(src),
            [
                ("services.web.image".to_string(), "nginx"),
                ("list.[]".to_string(), "a"),
                ("list.[]".to_string(), "b"),
                ("list.[].k".to_string(), "c"),
            ]
        );
    }

    #[test]
    fn spans_exclude_quotes_and_mappings_are_grouped() {
        let src = "include:\n  - project: \"a/b\"\n    ref: v1\n  - project: c/d\n";
        let s = scalars(src);
        assert_eq!(&src[s[0].span.clone()], "a/b");
        assert_eq!(s[0].mapping, s[1].mapping);
        assert_ne!(s[1].mapping, s[2].mapping);
    }

    #[test]
    fn anchors_tags_and_multiple_documents() {
        let src = "a: &x !!str v\n---\nb: [c]\n";
        assert_eq!(
            paths(src),
            [("a".to_string(), "v"), ("b.[]".to_string(), "c")]
        );
    }
}
