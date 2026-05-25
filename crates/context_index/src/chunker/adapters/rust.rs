use tree_sitter::Node;

use crate::chunker::adapter::{
    LanguageAdapter, collapsed_braced_signature, collect_identifier_references, generic_node_name,
    leading_sibling_comment_block, signature_until,
};

pub struct RustAdapter;

impl LanguageAdapter for RustAdapter {
    fn language(&self) -> tree_sitter::Language {
        tree_sitter_rust::LANGUAGE.into()
    }

    fn language_id(&self) -> &'static str {
        "rust"
    }

    fn definition_node_kinds(&self) -> &'static [&'static str] {
        &[
            "function_item",
            "impl_item",
            "struct_item",
            "enum_item",
            "trait_item",
            "mod_item",
            "union_item",
            "type_item",
        ]
    }

    fn block_child_kinds(&self) -> &'static [&'static str] {
        &["block", "declaration_list", "source_file", "match_block"]
    }

    fn import_node_kinds(&self) -> &'static [&'static str] {
        &["use_declaration"]
    }

    fn node_name(&self, node: Node<'_>, source: &[u8]) -> Option<String> {
        generic_node_name(node, source)
    }

    fn signature(&self, node: Node<'_>, source: &[u8]) -> String {
        signature_until(node, source, &["block", "declaration_list"], true)
    }

    fn leading_doc_comment(&self, node: Node<'_>, source: &[u8]) -> String {
        leading_sibling_comment_block(node, source, |text| {
            let stripped = text.trim_start();
            stripped.starts_with("///")
                || stripped.starts_with("//!")
                || (stripped.starts_with("/**") && !stripped.starts_with("/***"))
        })
    }

    fn collapse_definition(&self, node: Node<'_>, source: &[u8]) -> String {
        collapsed_braced_signature(&self.signature(node, source))
    }

    fn referenced_symbols(&self, node: Node<'_>, source: &[u8]) -> Vec<String> {
        collect_identifier_references(
            node,
            source,
            &["call_expression", "scoped_identifier", "field_expression"],
            &[
                "self", "Self", "Result", "Option", "Some", "None", "Ok", "Err",
            ],
            true,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_rust_name_signature_doc_and_symbols() {
        let adapter = RustAdapter;
        let mut parser = tree_sitter::Parser::new();
        let language = adapter.language();
        parser.set_language(&language).unwrap();
        let source = br#"/// Runs work.
fn run() {
    Worker::new().execute();
}
"#;
        let tree = parser.parse(source, None).unwrap();
        let root = tree.root_node();
        let mut cursor = root.walk();
        let function = root
            .named_children(&mut cursor)
            .find(|node| node.kind() == "function_item")
            .unwrap();
        assert_eq!(adapter.node_name(function, source).as_deref(), Some("run"));
        assert_eq!(adapter.signature(function, source), "fn run()");
        assert!(
            adapter
                .leading_doc_comment(function, source)
                .contains("Runs work")
        );
        assert!(
            adapter
                .referenced_symbols(function, source)
                .contains(&"Worker".to_string())
        );
    }
}
