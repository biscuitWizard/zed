use std::sync::Arc;

use tree_sitter::{Node, Parser};

use super::{
    FileChunker,
    adapter::{LanguageAdapter, node_text},
    budget::{ChunkBudget, nws_size},
    ids,
    text::TextChunker,
    types::{Chunk, ChunkView},
};

struct Definition<'tree> {
    node: Node<'tree>,
    kind: String,
    name: Option<String>,
    byte_start: usize,
    byte_end: usize,
    line_start: usize,
    line_end: usize,
    children: Vec<Definition<'tree>>,
}

impl<'tree> Definition<'tree> {
    fn new(node: Node<'tree>, kind: String, name: Option<String>) -> Self {
        Self {
            kind,
            name,
            byte_start: node.start_byte(),
            byte_end: node.end_byte(),
            line_start: node.start_position().row,
            line_end: node.end_position().row,
            node,
            children: Vec::new(),
        }
    }
}

pub struct AstChunker {
    budget: ChunkBudget,
    adapter: Arc<dyn LanguageAdapter>,
    text_fallback: TextChunker,
}

impl AstChunker {
    pub fn new(budget: ChunkBudget, adapter: Arc<dyn LanguageAdapter>) -> Self {
        let language_id = adapter.language_id();
        Self {
            budget,
            adapter,
            text_fallback: TextChunker {
                budget,
                language_id,
                doc_mode: false,
            },
        }
    }
}

impl FileChunker for AstChunker {
    fn chunk_file(&self, file_path: &str, bytes: &[u8]) -> Vec<Chunk> {
        let text = String::from_utf8_lossy(bytes);
        if text.trim().is_empty() {
            return Vec::new();
        }

        let mut parser = Parser::new();
        let language = self.adapter.language();
        if parser.set_language(&language).is_err() {
            return self.text_fallback.chunk_file(file_path, bytes);
        }

        let Some(tree) = parser.parse(bytes, None) else {
            return self.text_fallback.chunk_file(file_path, bytes);
        };

        let file_def = build_definition_tree(tree.root_node(), self.adapter.as_ref(), bytes);
        if file_def.children.is_empty() {
            log::debug!(
                "[context_index] ast yielded 0 defs for {file_path}, falling back to text windowing"
            );
            return self.text_fallback.chunk_file(file_path, bytes);
        }

        let imports = extract_imports(tree.root_node(), self.adapter.as_ref(), bytes);
        let mut chunks = Vec::new();
        for child in &file_def.children {
            walk_definition(
                child,
                None,
                &mut Vec::new(),
                file_path,
                self.adapter.as_ref(),
                bytes,
                &imports,
                self.budget,
                &mut chunks,
            );
        }
        chunks
    }
}

fn build_definition_tree<'tree>(
    root: Node<'tree>,
    adapter: &dyn LanguageAdapter,
    source: &[u8],
) -> Definition<'tree> {
    let mut file_def = Definition::new(root, "file".to_string(), None);
    collect_definitions(root, &mut file_def, adapter, source);
    file_def
}

fn collect_definitions<'tree>(
    node: Node<'tree>,
    current: &mut Definition<'tree>,
    adapter: &dyn LanguageAdapter,
    source: &[u8],
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if adapter.definition_node_kinds().contains(&child.kind()) {
            let mut definition = Definition::new(
                child,
                child.kind().to_string(),
                adapter.node_name(child, source),
            );
            collect_definitions(child, &mut definition, adapter, source);
            current.children.push(definition);
        } else {
            collect_definitions(child, current, adapter, source);
        }
    }
}

fn walk_definition<'tree>(
    definition: &Definition<'tree>,
    parent_id: Option<String>,
    ancestors: &mut Vec<String>,
    file_path: &str,
    adapter: &dyn LanguageAdapter,
    source: &[u8],
    imports: &str,
    budget: ChunkBudget,
    chunks: &mut Vec<Chunk>,
) {
    let emitted = emit_chunks_for_definition(
        definition,
        parent_id.as_deref(),
        ancestors,
        file_path,
        adapter,
        source,
        imports,
        budget,
    );

    let canonical_parent_id = emitted
        .iter()
        .find(|chunk| chunk.view == ChunkView::Code)
        .map(|chunk| chunk.node_id.clone())
        .or(parent_id);
    chunks.extend(emitted);

    ancestors.push(
        definition
            .name
            .clone()
            .unwrap_or_else(|| definition.kind.clone()),
    );
    for child in &definition.children {
        walk_definition(
            child,
            canonical_parent_id.clone(),
            ancestors,
            file_path,
            adapter,
            source,
            imports,
            budget,
            chunks,
        );
    }
    ancestors.pop();
}

fn emit_chunks_for_definition(
    definition: &Definition<'_>,
    parent_id: Option<&str>,
    ancestors: &[String],
    file_path: &str,
    adapter: &dyn LanguageAdapter,
    source: &[u8],
    imports: &str,
    budget: ChunkBudget,
) -> Vec<Chunk> {
    let intrinsic = intrinsic_text(definition, source, adapter);
    if intrinsic.trim().is_empty() {
        return Vec::new();
    }

    let breadcrumb = build_breadcrumb(file_path, ancestors, definition);
    let signature = adapter.signature(definition.node, source);
    let doc_text = adapter.leading_doc_comment(definition.node, source);
    let referenced_symbols = adapter.referenced_symbols(definition.node, source);

    let mut chunks = Vec::new();
    if nws_size(&intrinsic) <= budget.maximum {
        let embed_text = embed_text_code(
            file_path,
            adapter.language_id(),
            &breadcrumb,
            imports,
            &doc_text,
            &intrinsic,
        );
        chunks.push(make_chunk(
            file_path,
            adapter.language_id(),
            definition,
            definition.byte_start,
            definition.byte_end,
            definition.line_start,
            definition.line_end,
            parent_id,
            ChunkView::Code,
            intrinsic,
            doc_text.clone(),
            signature.clone(),
            breadcrumb.clone(),
            imports.to_string(),
            embed_text,
            referenced_symbols,
        ));
    } else {
        let ranges = block_split(definition.node, source, adapter, budget);
        let ranges = greedy_merge_ranges(&ranges, source, budget);
        let range_count = ranges.len();
        for (idx, (byte_start, byte_end)) in ranges.into_iter().enumerate() {
            let code_text = String::from_utf8_lossy(&source[byte_start..byte_end]).into_owned();
            if nws_size(&code_text) < budget.minimum && idx + 1 < range_count {
                continue;
            }
            let sub_doc = if idx == 0 {
                doc_text.clone()
            } else {
                String::new()
            };
            let embed_text = embed_text_code(
                file_path,
                adapter.language_id(),
                &breadcrumb,
                imports,
                &sub_doc,
                &code_text,
            );
            chunks.push(make_chunk(
                file_path,
                adapter.language_id(),
                definition,
                byte_start,
                byte_end,
                line_for_byte(source, byte_start),
                line_for_byte(source, byte_end.saturating_sub(1)),
                parent_id,
                ChunkView::Code,
                code_text,
                sub_doc,
                signature.clone(),
                breadcrumb.clone(),
                imports.to_string(),
                embed_text,
                Vec::new(),
            ));
        }
    }

    if !doc_text.is_empty() && !chunks.is_empty() && nws_size(&doc_text) >= budget.doc_view_min {
        let canonical = &chunks[0];
        let embed_text = embed_text_doc(
            file_path,
            adapter.language_id(),
            &breadcrumb,
            &signature,
            &doc_text,
        );
        chunks.push(make_chunk(
            file_path,
            adapter.language_id(),
            definition,
            canonical.byte_start as usize,
            canonical.byte_end as usize,
            canonical.line_start as usize,
            canonical.line_end as usize,
            parent_id,
            ChunkView::Doc,
            canonical.code_text.clone(),
            doc_text,
            signature,
            breadcrumb,
            imports.to_string(),
            embed_text,
            Vec::new(),
        ));
    }

    chunks
}

fn make_chunk(
    file_path: &str,
    language_id: &str,
    definition: &Definition<'_>,
    byte_start: usize,
    byte_end: usize,
    line_start: usize,
    line_end: usize,
    parent_id: Option<&str>,
    view: ChunkView,
    code_text: String,
    doc_text: String,
    signature: String,
    breadcrumb: String,
    imports: String,
    embed_text: String,
    referenced_symbols: Vec<String>,
) -> Chunk {
    let byte_start_i64 = byte_start as i64;
    let byte_end_i64 = byte_end as i64;
    let embed_text_sha = ids::embed_text_sha(&embed_text);
    Chunk {
        id: ids::chunk_id(file_path, byte_start_i64, byte_end_i64, view),
        node_id: ids::node_id(file_path, byte_start_i64, byte_end_i64),
        view,
        parent_id: parent_id.map(ToString::to_string),
        file_path: file_path.to_string(),
        language_id: language_id.to_string(),
        kind: definition.kind.clone(),
        name: definition.name.clone(),
        byte_start: byte_start_i64,
        byte_end: byte_end_i64,
        line_start: line_start as i32,
        line_end: line_end as i32,
        imports,
        signature,
        breadcrumb,
        code_text,
        doc_text,
        embed_text,
        embed_text_sha,
        referenced_symbols,
    }
}

fn intrinsic_text(
    definition: &Definition<'_>,
    source: &[u8],
    adapter: &dyn LanguageAdapter,
) -> String {
    if definition.children.is_empty() {
        return node_text(definition.node, source);
    }

    let mut out = String::new();
    let mut cursor = definition.byte_start;
    for child in &definition.children {
        if child.byte_start > cursor {
            out.push_str(&String::from_utf8_lossy(&source[cursor..child.byte_start]));
        }
        out.push_str(adapter.collapse_definition(child.node, source).trim_end());
        out.push('\n');
        cursor = child.byte_end;
    }
    if cursor < definition.byte_end {
        out.push_str(&String::from_utf8_lossy(
            &source[cursor..definition.byte_end],
        ));
    }
    out
}

fn extract_imports(root: Node<'_>, adapter: &dyn LanguageAdapter, source: &[u8]) -> String {
    let import_kinds = adapter.import_node_kinds();
    if import_kinds.is_empty() {
        return String::new();
    }

    let mut imports = Vec::new();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if import_kinds.contains(&child.kind()) {
            imports.push(node_text(child, source).trim_end().to_string());
        }
    }
    imports.join("\n")
}

fn block_split(
    node: Node<'_>,
    source: &[u8],
    adapter: &dyn LanguageAdapter,
    budget: ChunkBudget,
) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    block_split_inner(node, source, adapter, budget, &mut ranges);
    ranges
}

fn block_split_inner(
    node: Node<'_>,
    source: &[u8],
    adapter: &dyn LanguageAdapter,
    budget: ChunkBudget,
    ranges: &mut Vec<(usize, usize)>,
) {
    if nws_size(&node_text(node, source)) <= budget.maximum {
        ranges.push((node.start_byte(), node.end_byte()));
        return;
    }

    let mut recursed = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if adapter.block_child_kinds().contains(&child.kind()) || child.child_count() > 0 {
            recursed = true;
            block_split_inner(child, source, adapter, budget, ranges);
        }
    }

    if !recursed {
        ranges.push((node.start_byte(), node.end_byte()));
    }
}

pub(crate) fn greedy_merge_ranges(
    ranges: &[(usize, usize)],
    source: &[u8],
    budget: ChunkBudget,
) -> Vec<(usize, usize)> {
    if ranges.is_empty() {
        return Vec::new();
    }

    let mut merged = Vec::with_capacity(ranges.len());
    merged.push(ranges[0]);
    for (start, end) in ranges.iter().copied().skip(1) {
        let last = merged.last_mut().expect("merged is non-empty");
        let combined = String::from_utf8_lossy(&source[last.0..end]);
        if nws_size(&combined) <= budget.target {
            last.1 = end;
        } else {
            merged.push((start, end));
        }
    }
    merged
}

fn build_breadcrumb(file_path: &str, ancestors: &[String], definition: &Definition<'_>) -> String {
    let mut parts = vec![file_path.to_string()];
    for ancestor in ancestors {
        parts.push(ancestor.clone());
    }
    parts.push(
        definition
            .name
            .clone()
            .unwrap_or_else(|| definition.kind.clone()),
    );
    parts.join(" > ")
}

fn embed_text_code(
    file_path: &str,
    language_id: &str,
    breadcrumb: &str,
    imports: &str,
    doc_text: &str,
    code_text: &str,
) -> String {
    let mut parts = vec![
        format!("# {file_path}"),
        format!("# language: {language_id}"),
        format!("# {breadcrumb}"),
    ];
    if !imports.is_empty() {
        parts.push(imports.to_string());
    }
    if !doc_text.is_empty() {
        parts.push(doc_text.to_string());
    }
    parts.push(code_text.to_string());
    parts.join("\n").trim().to_string() + "\n"
}

fn embed_text_doc(
    file_path: &str,
    language_id: &str,
    breadcrumb: &str,
    signature: &str,
    doc_text: &str,
) -> String {
    [
        format!("# {file_path}"),
        format!("# language: {language_id}"),
        format!("# {breadcrumb}"),
        "# (doc-view; reranked against the code body)".to_string(),
        String::new(),
        signature.to_string(),
        String::new(),
        doc_text.to_string(),
    ]
    .join("\n")
    .trim()
    .to_string()
        + "\n"
}

fn line_for_byte(source: &[u8], byte: usize) -> usize {
    source[..byte.min(source.len())]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::adapter::{collapsed_braced_signature, generic_node_name, signature_until};

    struct TestAdapter;

    impl LanguageAdapter for TestAdapter {
        fn language(&self) -> tree_sitter::Language {
            tree_sitter_rust::LANGUAGE.into()
        }

        fn language_id(&self) -> &'static str {
            "rust"
        }

        fn definition_node_kinds(&self) -> &'static [&'static str] {
            &["function_item", "struct_item"]
        }

        fn block_child_kinds(&self) -> &'static [&'static str] {
            &["block", "source_file"]
        }

        fn import_node_kinds(&self) -> &'static [&'static str] {
            &["use_declaration"]
        }

        fn node_name(&self, node: Node<'_>, source: &[u8]) -> Option<String> {
            generic_node_name(node, source)
        }

        fn signature(&self, node: Node<'_>, source: &[u8]) -> String {
            signature_until(node, source, &["block", "field_declaration_list"], true)
        }

        fn leading_doc_comment(&self, _node: Node<'_>, _source: &[u8]) -> String {
            "/// This function has enough documentation to get a doc row.".to_string()
        }

        fn collapse_definition(&self, node: Node<'_>, source: &[u8]) -> String {
            collapsed_braced_signature(&self.signature(node, source))
        }
    }

    #[test]
    fn intrinsic_text_masks_children() {
        let chunker = AstChunker::new(
            ChunkBudget::from_token_counts(128, 256, 8),
            Arc::new(TestAdapter),
        );
        let chunks = chunker.chunk_file(
            "src/lib.rs",
            b"struct Outer { value: i32 }\nfn child() { println!(\"hi\"); }\n",
        );
        assert!(chunks.iter().any(|chunk| chunk.kind == "struct_item"));
        assert!(chunks.iter().any(|chunk| chunk.kind == "function_item"));
    }

    #[test]
    fn doc_view_emitted_only_above_threshold() {
        let chunker = AstChunker::new(
            ChunkBudget::from_token_counts_with_doc_view_min(128, 256, 8, 4),
            Arc::new(TestAdapter),
        );
        let chunks = chunker.chunk_file("src/lib.rs", b"fn child() { println!(\"hi\"); }\n");
        assert!(chunks.iter().any(|chunk| chunk.view == ChunkView::Doc));
    }

    #[test]
    fn greedy_merge_keeps_under_target() {
        let budget = ChunkBudget::from_token_counts(4, 64, 1);
        let source = b"aa\nbb\ncccccccccccccccc\n";
        let merged = greedy_merge_ranges(&[(0, 3), (3, 6), (6, source.len())], source, budget);
        assert_eq!(merged[0], (0, 6));
        assert_eq!(merged[1], (6, source.len()));
    }

    #[test]
    fn zero_defs_falls_back_to_text_chunker() {
        let chunker = AstChunker::new(
            ChunkBudget::from_token_counts(16, 32, 1),
            Arc::new(TestAdapter),
        );
        let chunks = chunker.chunk_file("src/lib.rs", b"let this_is_not_item_syntax = ;\n");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].kind, "file");
    }
}
