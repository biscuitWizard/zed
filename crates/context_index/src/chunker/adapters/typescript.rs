use tree_sitter::Node;

use crate::chunker::{
    adapter::{
        LanguageAdapter, collapsed_braced_signature, collect_identifier_references,
        leading_sibling_comment_block, signature_until,
    },
    adapters::javascript::name_from_js_like_node,
};

#[derive(Clone, Copy)]
pub enum TypeScriptVariant {
    TypeScript,
    Tsx,
}

pub struct TypeScriptAdapter {
    variant: TypeScriptVariant,
    language_id: &'static str,
}

impl TypeScriptAdapter {
    pub fn new(variant: TypeScriptVariant, language_id: &'static str) -> Self {
        Self {
            variant,
            language_id,
        }
    }
}

impl LanguageAdapter for TypeScriptAdapter {
    fn language(&self) -> tree_sitter::Language {
        match self.variant {
            TypeScriptVariant::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            TypeScriptVariant::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }

    fn language_id(&self) -> &'static str {
        self.language_id
    }

    fn definition_node_kinds(&self) -> &'static [&'static str] {
        &[
            "function_declaration",
            "class_declaration",
            "method_definition",
            "lexical_declaration",
            "interface_declaration",
            "type_alias_declaration",
            "enum_declaration",
        ]
    }

    fn block_child_kinds(&self) -> &'static [&'static str] {
        &["statement_block", "class_body", "program", "object_type"]
    }

    fn import_node_kinds(&self) -> &'static [&'static str] {
        &["import_statement"]
    }

    fn node_name(&self, node: Node<'_>, source: &[u8]) -> Option<String> {
        name_from_js_like_node(node, source)
    }

    fn signature(&self, node: Node<'_>, source: &[u8]) -> String {
        signature_until(
            node,
            source,
            &["statement_block", "class_body", "object_type"],
            true,
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_typescript_interface_and_doc() {
        let adapter = TypeScriptAdapter::new(TypeScriptVariant::TypeScript, "typescript");
        let mut parser = tree_sitter::Parser::new();
        let language = adapter.language();
        parser.set_language(&language).unwrap();
        let source = br#"/** Public shape. */
interface Shape {
  draw(): void
}
"#;
        let tree = parser.parse(source, None).unwrap();
        let root = tree.root_node();
        let mut cursor = root.walk();
        let interface = root
            .named_children(&mut cursor)
            .find(|node| node.kind() == "interface_declaration")
            .unwrap();
        assert_eq!(
            adapter.node_name(interface, source).as_deref(),
            Some("Shape")
        );
        assert!(
            adapter
                .signature(interface, source)
                .starts_with("interface Shape")
        );
        assert!(
            adapter
                .leading_doc_comment(interface, source)
                .contains("Public shape")
        );
    }
}
