use tree_sitter::Node;

use crate::chunker::adapter::{
    LanguageAdapter, collapsed_braced_signature, generic_node_name, node_text, signature_until,
};

pub struct PythonAdapter;

impl LanguageAdapter for PythonAdapter {
    fn language(&self) -> tree_sitter::Language {
        tree_sitter_python::LANGUAGE.into()
    }

    fn language_id(&self) -> &'static str {
        "python"
    }

    fn definition_node_kinds(&self) -> &'static [&'static str] {
        &[
            "function_definition",
            "class_definition",
            "decorated_definition",
        ]
    }

    fn block_child_kinds(&self) -> &'static [&'static str] {
        &[
            "block",
            "module",
            "if_statement",
            "for_statement",
            "while_statement",
            "try_statement",
        ]
    }

    fn import_node_kinds(&self) -> &'static [&'static str] {
        &["import_statement", "import_from_statement"]
    }

    fn node_name(&self, node: Node<'_>, source: &[u8]) -> Option<String> {
        generic_node_name(node, source)
    }

    fn signature(&self, node: Node<'_>, source: &[u8]) -> String {
        signature_until(node, source, &["block"], true)
    }

    fn leading_doc_comment(&self, node: Node<'_>, source: &[u8]) -> String {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() != "block" {
                continue;
            }
            let mut block_cursor = child.walk();
            let Some(first) = child.named_children(&mut block_cursor).next() else {
                return String::new();
            };
            if first.kind() != "expression_statement" {
                return String::new();
            }
            let mut expression_cursor = first.walk();
            for candidate in first.named_children(&mut expression_cursor) {
                if candidate.kind() == "string" {
                    return node_text(candidate, source);
                }
            }
        }
        String::new()
    }

    fn collapse_definition(&self, node: Node<'_>, source: &[u8]) -> String {
        collapsed_braced_signature(&self.signature(node, source))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_python_name_signature_and_docstring() {
        let adapter = PythonAdapter;
        let mut parser = tree_sitter::Parser::new();
        let language = adapter.language();
        parser.set_language(&language).unwrap();
        let source = br#"def greet(name):
    """Say hello."""
    return f"hi {name}"
"#;
        let tree = parser.parse(source, None).unwrap();
        let root = tree.root_node();
        let mut cursor = root.walk();
        let function = root.named_children(&mut cursor).next().unwrap();
        assert_eq!(
            adapter.node_name(function, source).as_deref(),
            Some("greet")
        );
        assert_eq!(adapter.signature(function, source), "def greet(name):");
        assert!(
            adapter
                .leading_doc_comment(function, source)
                .contains("Say hello")
        );
        assert!(
            adapter
                .collapse_definition(function, source)
                .contains("def greet")
        );
    }
}
