use tree_sitter::Node;

use crate::chunker::adapter::{
    LanguageAdapter, collapsed_braced_signature, collapsed_statement_signature,
    collect_identifier_references, generic_node_name, leading_sibling_comment_block,
    signature_until,
};

pub struct CSharpAdapter;

impl LanguageAdapter for CSharpAdapter {
    fn language(&self) -> tree_sitter::Language {
        tree_sitter_c_sharp::LANGUAGE.into()
    }

    fn language_id(&self) -> &'static str {
        "csharp"
    }

    fn definition_node_kinds(&self) -> &'static [&'static str] {
        &[
            "namespace_declaration",
            "file_scoped_namespace_declaration",
            "class_declaration",
            "struct_declaration",
            "interface_declaration",
            "record_declaration",
            "record_struct_declaration",
            "enum_declaration",
            "method_declaration",
            "constructor_declaration",
            "destructor_declaration",
            "property_declaration",
            "indexer_declaration",
            "event_declaration",
            "event_field_declaration",
            "delegate_declaration",
            "operator_declaration",
            "conversion_operator_declaration",
        ]
    }

    fn block_child_kinds(&self) -> &'static [&'static str] {
        &[
            "block",
            "declaration_list",
            "accessor_list",
            "switch_body",
            "expression_statement",
            "if_statement",
            "for_statement",
            "foreach_statement",
            "while_statement",
            "do_statement",
            "try_statement",
            "switch_statement",
            "using_statement",
            "lock_statement",
            "return_statement",
            "throw_statement",
            "yield_statement",
            "break_statement",
            "continue_statement",
            "local_declaration_statement",
            "local_function_statement",
        ]
    }

    fn import_node_kinds(&self) -> &'static [&'static str] {
        &["using_directive", "extern_alias_directive"]
    }

    fn node_name(&self, node: Node<'_>, source: &[u8]) -> Option<String> {
        generic_node_name(node, source)
    }

    fn signature(&self, node: Node<'_>, source: &[u8]) -> String {
        signature_until(
            node,
            source,
            &[
                "block",
                "declaration_list",
                "accessor_list",
                "enum_member_declaration_list",
            ],
            true,
        )
    }

    fn leading_doc_comment(&self, node: Node<'_>, source: &[u8]) -> String {
        leading_sibling_comment_block(node, source, |text| {
            let stripped = text.trim_start();
            stripped.starts_with("///")
                || (stripped.starts_with("/**") && !stripped.starts_with("/***"))
        })
    }

    fn collapse_definition(&self, node: Node<'_>, source: &[u8]) -> String {
        let signature = self.signature(node, source);
        match node.kind() {
            "namespace_declaration"
            | "class_declaration"
            | "struct_declaration"
            | "interface_declaration"
            | "record_declaration"
            | "record_struct_declaration"
            | "enum_declaration"
            | "method_declaration"
            | "constructor_declaration"
            | "destructor_declaration"
            | "operator_declaration"
            | "conversion_operator_declaration"
            | "indexer_declaration"
            | "property_declaration"
            | "event_declaration" => collapsed_braced_signature(&signature),
            _ => collapsed_statement_signature(&signature),
        }
    }

    fn referenced_symbols(&self, node: Node<'_>, source: &[u8]) -> Vec<String> {
        collect_identifier_references(
            node,
            source,
            &[
                "invocation_expression",
                "member_access_expression",
                "object_creation_expression",
                "generic_name",
            ],
            &[
                "this",
                "value",
                "var",
                "null",
                "true",
                "false",
                "base",
                "string",
                "int",
                "float",
                "double",
                "bool",
                "void",
                "Debug",
                "Log",
                "Exception",
            ],
            true,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_csharp_name_signature_doc_and_symbols() {
        let adapter = CSharpAdapter;
        let mut parser = tree_sitter::Parser::new();
        let language = adapter.language();
        parser.set_language(&language).unwrap();
        let source = br#"using System;
class Player {
    /// Moves the player.
    public void Move() { Controller.Run(); }
}
"#;
        let tree = parser.parse(source, None).unwrap();
        let root = tree.root_node();
        let mut cursor = root.walk();
        let class = root
            .named_children(&mut cursor)
            .find(|node| node.kind() == "class_declaration")
            .unwrap();
        assert_eq!(adapter.node_name(class, source).as_deref(), Some("Player"));
        assert!(adapter.signature(class, source).starts_with("class Player"));
        let method = {
            let mut class_cursor = class.walk();
            class
                .named_children(&mut class_cursor)
                .flat_map(|node| {
                    let mut cursor = node.walk();
                    node.named_children(&mut cursor).collect::<Vec<_>>()
                })
                .find(|node| node.kind() == "method_declaration")
                .unwrap()
        };
        assert!(
            adapter
                .leading_doc_comment(method, source)
                .contains("Moves the player")
        );
        assert!(
            adapter
                .referenced_symbols(method, source)
                .contains(&"Controller".to_string())
        );
    }
}
