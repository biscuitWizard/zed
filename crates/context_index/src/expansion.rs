use anyhow::Result;

use crate::{
    chunker::{
        adapter::{LanguageAdapter, render_default_skeleton},
        adapters::{
            csharp::CSharpAdapter,
            go::GoAdapter,
            javascript::JavaScriptAdapter,
            python::PythonAdapter,
            rust::RustAdapter,
            typescript::{TypeScriptAdapter, TypeScriptVariant},
        },
    },
    store::{ChunkRow, LanceStore},
};

#[derive(Debug, Clone)]
pub struct ExpandedContext {
    pub matched: ChunkRow,
    pub ancestors: Vec<ChunkRow>,
    pub siblings: Vec<ChunkRow>,
    pub skeleton: String,
}

pub async fn expand_context(store: &LanceStore, mut matched: ChunkRow) -> Result<ExpandedContext> {
    if matched.view != "code"
        && let Some(canonical) = store
            .get_canonical_by_node_ids(&[matched.node_id.clone()])
            .await?
            .remove(&matched.node_id)
    {
        matched = canonical;
    }

    let ancestors = store.parent_chain(&matched).await?;
    let siblings = store.siblings_of(&matched).await?;
    let mut skeleton = if let Some(adapter) = adapter_for_language(&matched.language_id) {
        adapter.render_skeleton(&ancestors, &matched, &siblings)
    } else {
        render_default_skeleton(&ancestors, &matched, &siblings)
    };

    let symrefs = render_symref_section(store, &matched, 3).await?;
    if !symrefs.is_empty() {
        skeleton = format!("{}\n{}\n", skeleton.trim_end(), symrefs);
    }

    Ok(ExpandedContext {
        matched,
        ancestors,
        siblings,
        skeleton,
    })
}

async fn render_symref_section(
    store: &LanceStore,
    matched: &ChunkRow,
    max_refs: usize,
) -> Result<String> {
    if matched.referenced_symbols.is_empty() {
        return Ok(String::new());
    }

    let mut lines = Vec::new();
    for symbol in matched.referenced_symbols.iter().take(max_refs) {
        let rows = store.fts_search(symbol, 3, Some("code")).await?;
        if let Some(row) = rows
            .into_iter()
            .find(|row| row.name.as_deref() == Some(symbol.as_str()) && row.id != matched.id)
        {
            let signature = if row.signature.trim().is_empty() {
                row.name.as_deref().unwrap_or(row.kind.as_str()).to_string()
            } else {
                row.signature
            };
            lines.push(format!(
                "//   {}:{}-{}",
                row.file_path,
                row.line_start + 1,
                row.line_end + 1
            ));
            lines.push(format!("//   {signature}"));
        }
    }

    if lines.is_empty() {
        Ok(String::new())
    } else {
        Ok(format!("// 1-hop references:\n{}", lines.join("\n")))
    }
}

fn adapter_for_language(language_id: &str) -> Option<Box<dyn LanguageAdapter>> {
    match language_id {
        "csharp" => Some(Box::new(CSharpAdapter)),
        "go" => Some(Box::new(GoAdapter)),
        "javascript" | "javascriptreact" => Some(Box::new(JavaScriptAdapter)),
        "python" => Some(Box::new(PythonAdapter)),
        "rust" => Some(Box::new(RustAdapter)),
        "typescript" => Some(Box::new(TypeScriptAdapter::new(
            TypeScriptVariant::TypeScript,
            "typescript",
        ))),
        _ => None,
    }
}
