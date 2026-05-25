use tree_sitter::Node;

use crate::chunker::adapter::{
    LanguageAdapter, child_text_by_field, collapsed_braced_signature,
    collect_identifier_references, generic_node_name, leading_sibling_comment_block,
    signature_until,
};

pub struct JavaScriptAdapter;

impl LanguageAdapter for JavaScriptAdapter {
    fn language(&self) -> tree_sitter::Language {
        tree_sitter_javascript::LANGUAGE.into()
    }

    fn language_id(&self) -> &'static str {
        "javascript"
    }

    fn definition_node_kinds(&self) -> &'static [&'static str] {
        &[
            "function_declaration",
            "class_declaration",
            "method_definition",
            "lexical_declaration",
        ]
    }

    fn block_child_kinds(&self) -> &'static [&'static str] {
        &["statement_block", "class_body", "program"]
    }

    fn import_node_kinds(&self) -> &'static [&'static str] {
        &["import_statement"]
    }

    fn node_name(&self, node: Node<'_>, source: &[u8]) -> Option<String> {
        name_from_js_like_node(node, source)
    }

    fn signature(&self, node: Node<'_>, source: &[u8]) -> String {
        signature_until(node, source, &["statement_block", "class_body"], true)
    }

    fn leading_doc_comment(&self, node: Node<'_>, source: &[u8]) -> String {
        leading_sibling_comment_block(node, source, |text| text.trim_start().starts_with("/**"))
    }

    fn collapse_definition(&self, node: Node<'_>, source: &[u8]) -> String {
        collapsed_braced_signature(&self.signature(node, source))
    }

    fn referenced_symbols(&self, node: Node<'_>, source: &[u8]) -> Vec<String> {
        collect_identifier_references(
            node,
            source,
            &["call_expression", "member_expression", "new_expression"],
            &[
                "this",
                "undefined",
                "null",
                "true",
                "false",
                "console",
                "log",
            ],
            true,
        )
    }
}

pub(super) fn name_from_js_like_node(node: Node<'_>, source: &[u8]) -> Option<String> {
    child_text_by_field(node, source, "name").or_else(|| generic_node_name(node, source))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_javascript_name_signature_doc_and_symbols() {
        let adapter = JavaScriptAdapter;
        let mut parser = tree_sitter::Parser::new();
        let language = adapter.language();
        parser.set_language(&language).unwrap();
        let source = br#"/** Starts work. */
function start() {
  Worker.run();
}
"#;
        let tree = parser.parse(source, None).unwrap();
        let root = tree.root_node();
        let mut cursor = root.walk();
        let function = root
            .named_children(&mut cursor)
            .find(|node| node.kind() == "function_declaration")
            .unwrap();
        assert_eq!(
            adapter.node_name(function, source).as_deref(),
            Some("start")
        );
        assert_eq!(adapter.signature(function, source), "function start()");
        assert!(
            adapter
                .leading_doc_comment(function, source)
                .contains("Starts work")
        );
        assert!(
            adapter
                .referenced_symbols(function, source)
                .contains(&"Worker".to_string())
        );
    }
}
