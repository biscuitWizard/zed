use tree_sitter::Node;

use crate::chunker::adapter::{
    LanguageAdapter, collapsed_braced_signature, generic_node_name, leading_sibling_comment_block,
    signature_until,
};

pub struct GoAdapter;

impl LanguageAdapter for GoAdapter {
    fn language(&self) -> tree_sitter::Language {
        tree_sitter_go::LANGUAGE.into()
    }

    fn language_id(&self) -> &'static str {
        "go"
    }

    fn definition_node_kinds(&self) -> &'static [&'static str] {
        &[
            "function_declaration",
            "method_declaration",
            "type_declaration",
        ]
    }

    fn block_child_kinds(&self) -> &'static [&'static str] {
        &[
            "block",
            "source_file",
            "expression_switch_statement",
            "select_statement",
        ]
    }

    fn import_node_kinds(&self) -> &'static [&'static str] {
        &["import_declaration"]
    }

    fn node_name(&self, node: Node<'_>, source: &[u8]) -> Option<String> {
        generic_node_name(node, source)
    }

    fn signature(&self, node: Node<'_>, source: &[u8]) -> String {
        signature_until(node, source, &["block"], true)
    }

    fn leading_doc_comment(&self, node: Node<'_>, source: &[u8]) -> String {
        leading_sibling_comment_block(node, source, |text| text.trim_start().starts_with("//"))
    }

    fn collapse_definition(&self, node: Node<'_>, source: &[u8]) -> String {
        collapsed_braced_signature(&self.signature(node, source))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_go_name_signature_and_doc() {
        let adapter = GoAdapter;
        let mut parser = tree_sitter::Parser::new();
        let language = adapter.language();
        parser.set_language(&language).unwrap();
        let source = br#"package main

// Run starts work.
func Run() {
}
"#;
        let tree = parser.parse(source, None).unwrap();
        let root = tree.root_node();
        let mut cursor = root.walk();
        let function = root
            .named_children(&mut cursor)
            .find(|node| node.kind() == "function_declaration")
            .unwrap();
        assert_eq!(adapter.node_name(function, source).as_deref(), Some("Run"));
        assert_eq!(adapter.signature(function, source), "func Run()");
        assert!(
            adapter
                .leading_doc_comment(function, source)
                .contains("Run starts")
        );
    }
}
