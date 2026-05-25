use tree_sitter::Node;

pub trait LanguageAdapter: Send + Sync {
    fn language(&self) -> tree_sitter::Language;
    fn language_id(&self) -> &'static str;
    fn definition_node_kinds(&self) -> &'static [&'static str];
    fn block_child_kinds(&self) -> &'static [&'static str];
    fn import_node_kinds(&self) -> &'static [&'static str];
    fn node_name(&self, node: Node<'_>, source: &[u8]) -> Option<String>;
    fn signature(&self, node: Node<'_>, source: &[u8]) -> String;
    fn leading_doc_comment(&self, node: Node<'_>, source: &[u8]) -> String;
    fn collapse_definition(&self, node: Node<'_>, source: &[u8]) -> String;

    fn referenced_symbols(&self, _node: Node<'_>, _source: &[u8]) -> Vec<String> {
        Vec::new()
    }
}

pub fn node_text(node: Node<'_>, source: &[u8]) -> String {
    String::from_utf8_lossy(&source[node.start_byte()..node.end_byte()]).into_owned()
}

pub fn compact_lines(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn child_text_by_field(node: Node<'_>, source: &[u8], field_name: &str) -> Option<String> {
    node.child_by_field_name(field_name)
        .map(|child| node_text(child, source))
        .filter(|text| !text.trim().is_empty())
}

pub fn first_descendant_text(
    node: Node<'_>,
    source: &[u8],
    candidate_kinds: &[&str],
) -> Option<String> {
    if candidate_kinds.contains(&node.kind()) {
        let text = node_text(node, source);
        if !text.trim().is_empty() {
            return Some(text);
        }
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(text) = first_descendant_text(child, source, candidate_kinds) {
            return Some(text);
        }
    }

    None
}

pub fn generic_node_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    child_text_by_field(node, source, "name").or_else(|| {
        first_descendant_text(
            node,
            source,
            &[
                "identifier",
                "type_identifier",
                "field_identifier",
                "property_identifier",
                "qualified_name",
            ],
        )
    })
}

pub fn signature_until(
    node: Node<'_>,
    source: &[u8],
    cutoff_kinds: &[&str],
    strip_trailing_semicolon: bool,
) -> String {
    let mut end = node.end_byte();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if cutoff_kinds.contains(&child.kind()) {
            end = child.start_byte();
            break;
        }
    }

    let mut signature = compact_lines(&String::from_utf8_lossy(&source[node.start_byte()..end]));
    if strip_trailing_semicolon {
        signature = signature.trim_end_matches(';').trim_end().to_string();
    }
    signature
}

pub fn collapsed_braced_signature(signature: &str) -> String {
    if signature.is_empty() {
        "{ /* ... */ }".to_string()
    } else {
        format!("{signature} {{ /* ... */ }}")
    }
}

pub fn collapsed_statement_signature(signature: &str) -> String {
    if signature.is_empty() {
        "/* ... */".to_string()
    } else {
        format!("{signature};")
    }
}

pub fn leading_sibling_comment_block(
    node: Node<'_>,
    source: &[u8],
    is_doc_comment: impl Fn(&str) -> bool,
) -> String {
    let Some(parent) = node.parent() else {
        return String::new();
    };

    let mut cursor = parent.walk();
    let siblings = parent.children(&mut cursor).collect::<Vec<_>>();
    let Some(idx) = siblings.iter().position(|sibling| {
        sibling.start_byte() == node.start_byte()
            && sibling.end_byte() == node.end_byte()
            && sibling.kind() == node.kind()
    }) else {
        return String::new();
    };

    let mut comments = Vec::new();
    for sibling in siblings[..idx].iter().rev() {
        if sibling.kind() != "comment"
            && sibling.kind() != "line_comment"
            && sibling.kind() != "block_comment"
        {
            break;
        }
        let text = node_text(*sibling, source);
        if !is_doc_comment(&text) {
            break;
        }
        comments.push(text.trim_end().to_string());
    }

    comments.reverse();
    comments.join("\n")
}

pub fn collect_identifier_references(
    node: Node<'_>,
    source: &[u8],
    parent_kinds: &[&str],
    stop_symbols: &[&str],
    uppercase_only: bool,
) -> Vec<String> {
    let mut seen = Vec::<String>::new();
    collect_identifier_references_inner(
        node,
        source,
        parent_kinds,
        stop_symbols,
        uppercase_only,
        &mut seen,
    );
    seen.truncate(32);
    seen
}

fn collect_identifier_references_inner(
    node: Node<'_>,
    source: &[u8],
    parent_kinds: &[&str],
    stop_symbols: &[&str],
    uppercase_only: bool,
    seen: &mut Vec<String>,
) {
    if node.kind() == "identifier"
        && node
            .parent()
            .is_some_and(|parent| parent_kinds.contains(&parent.kind()))
    {
        let text = node_text(node, source);
        let starts_uppercase = text
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_uppercase());
        if text.len() > 1
            && !stop_symbols.contains(&text.as_str())
            && (!uppercase_only || starts_uppercase)
            && !seen.iter().any(|existing| existing == &text)
        {
            seen.push(text);
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_identifier_references_inner(
            child,
            source,
            parent_kinds,
            stop_symbols,
            uppercase_only,
            seen,
        );
    }
}
