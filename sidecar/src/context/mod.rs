//! Assemble the `repository_context` string sent to Phoenix. This is where
//! *context selection* lives: which symbols/imports make it into the prompt.
//!
//! To *change* what gets picked, edit [`build_repository_context`]. To change
//! the *ranking* heuristic (which files are even considered), see
//! `crate::index::Index` and the way `resolve.rs` maps `import ... from "./y"`.

mod fmt;
mod resolve;

pub use resolve::{
    resolve_c_include, resolve_go_import, resolve_import, resolve_import_for,
    resolve_java_import, resolve_python_import, resolve_rust_import,
};

use crate::index::Index;
use fmt::fmt_symbol;
use std::path::PathBuf;
use tree_sitter::{Node, Tree};

/// Byte cap on the final `repository_context` string. Keeps prompt size sane.
pub const REPO_CTX_CAP: usize = 2000;

/// Bleed-off token-per-byte heuristic from Zed's `cursor_excerpt.rs`. Used only to
/// size the cursor excerpt: ~3 bytes per token matches editor text well enough.
pub const BYTES_PER_TOKEN_GUESS: usize = 3;

/// Cursor-centered slice of the current buffer, returned with cursor position
/// projected into it. Byte offsets are into the *source* text.
pub struct Excerpt {
    pub start_byte: usize,
    pub end_byte: usize,
    pub cursor_offset_in_excerpt: usize,
}

/// Smart cursor excerpt.
///
/// Tries syntax-aware expansion first: walk the ancestor chain at `cursor_offset`,
/// pick the largest ancestor whose byte range fits `token_budget × 3` bytes.
/// Falls back to linewise symmetric expansion
///
/// `token_budget` is a *token* budget; byte-cap = `token_budget × 3`.
pub fn compute_cursor_excerpt(
    text: &str,
    tree: Option<&Tree>,
    cursor_offset: usize,
    token_budget: usize,
) -> Excerpt {
    let byte_budget = token_budget.saturating_mul(BYTES_PER_TOKEN_GUESS);
    let cursor = cursor_offset.min(text.len());
    if text.is_empty() || byte_budget == 0 {
        return Excerpt {
            start_byte: 0,
            end_byte: text.len(),
            cursor_offset_in_excerpt: cursor,
        };
    }
    if text.len() <= byte_budget {
        return Excerpt {
            start_byte: 0,
            end_byte: text.len(),
            cursor_offset_in_excerpt: cursor,
        };
    }
    if let Some(t) = tree {
        if let Some(range) = syntax_expansion(text, t, cursor, byte_budget) {
            return Excerpt {
                start_byte: range.0,
                end_byte: range.1,
                cursor_offset_in_excerpt: cursor - range.0,
            };
        }
    }
    let (s, e) = linewise_symmetric_expansion(text, cursor, byte_budget);
    Excerpt {
        start_byte: s,
        end_byte: e,
        cursor_offset_in_excerpt: cursor - s,
    }
}

/// Find the largest syntax ancestor of `cursor` whose byte range fits the budget.
/// Returns `(start, end)` snapped to line boundaries so excerpts read like code,
/// not fragments. `None` if even the cursor's innermost block overflows the budget.
fn syntax_expansion(
    text: &str,
    tree: &Tree,
    cursor: usize,
    byte_budget: usize,
) -> Option<(usize, usize)> {
    let probe = cursor.saturating_sub(1).min(text.len());
    let mut node = tree
        .root_node()
        .descendant_for_byte_range(probe, probe)?;
    let mut best: Option<(usize, usize)> = None;
    loop {
        let s = node.start_byte();
        let e = node.end_byte().min(text.len());
        // Snap to line boundaries for a cleaner prompt. Clamping to the line
        // containing the node's first/last byte means we never widen past what
        // fits, only past what the AST reports (see also Zed's `compute_syntax_ranges`).
        let ls = text[..s].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let le = text[e..]
            .find('\n')
            .map(|i| e + i + 1)
            .unwrap_or(text.len());
        if le.saturating_sub(ls) > byte_budget {
            break;
        }
        best = Some((ls, le));
        node = node.parent()?;
    }
    best
}

/// Alternating down/up line expansion. Charges each line `max(1, bytes/3)` tokens
/// against a token budget of `byte_budget / 3`. Deterministic, O(lines_added).
pub fn linewise_symmetric_expansion(text: &str, cursor: usize, byte_budget: usize) -> (usize, usize) {
    // Cursor may sit mid-line: treat "line at cursor" as our seed.
    let cur_line_start = text[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let cur_line_end = text[cursor..]
        .find('\n')
        .map(|i| cursor + i + 1)
        .unwrap_or(text.len());

    fn line_cost(line: &str) -> usize {
        // Min 1 token per line so empty lines aren't free-forever.
        (line.len() / BYTES_PER_TOKEN_GUESS).max(1)
    }

    let token_budget = byte_budget / BYTES_PER_TOKEN_GUESS;
    let mut start = cur_line_start;
    let mut end = cur_line_end;
    let mut spent = line_cost(&text[cur_line_start..cur_line_end]);
    if spent > token_budget {
        return (start, end);
    }
    loop {
        // Try expanding down first.
        let next_down = if end < text.len() {
            let le = text[end..]
                .find('\n')
                .map(|i| end + i + 1)
                .unwrap_or(text.len());
            Some((end, le))
        } else {
            None
        };
        let mut advanced = false;
        if let Some((ds, de)) = next_down {
            let c = line_cost(&text[ds..de]);
            if spent + c <= token_budget {
                end = de;
                spent += c;
                advanced = true;
            }
        }
        // Then up.
        if start > 0 {
            let prev_end = start.saturating_sub(1); // one-past-end of prev line
            let prev_start = text[..prev_end].rfind('\n').map(|i| i + 1).unwrap_or(0);
            let c = line_cost(&text[prev_start..start]);
            if spent + c <= token_budget {
                start = prev_start;
                spent += c;
                advanced = true;
            }
        }
        if !advanced {
            break;
        }
        if spent >= token_budget {
            break;
        }
    }
    (start, end)
}

/// Pull identifier-ish tokens from the lines surrounding the cursor (±3).
/// Result feeds BM25 ranking. Deduped, capped at 12.
pub fn identifiers_near_cursor(text: &str, cursor_offset: usize, radius_lines: usize) -> Vec<String> {
    let cursor = cursor_offset.min(text.len());
    // Walk back `radius_lines` line-starts.
    let mut start = cursor;
    for _ in 0..=radius_lines {
        match text[..start].rfind('\n') {
            Some(i) => start = i,
            None => {
                start = 0;
                break;
            }
        }
        if start == 0 {
            break;
        }
    }
    let mut end = cursor;
    for _ in 0..=radius_lines {
        match text[end..].find('\n') {
            Some(i) => end += i + 1,
            None => {
                end = text.len();
                break;
            }
        }
        if end >= text.len() {
            end = text.len();
            break;
        }
    }
    let toks = crate::index::tokenize(&text[start..end]);
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for t in toks {
        // Skip one-char tokens (x, y, i) and language keywords. ponytail:
        // not a real stoplist — add one if prompts start pulling irrelevant matches.
        if t.len() < 2 || matches!(
            t.as_str(),
            "use" | "in" | "is" | "of" | "on" | "if" | "to" | "by" | "as" | "at" | "fn"
                | "let" | "const" | "var" | "return" | "function" | "import" | "export"
                | "class" | "interface" | "type" | "new"
        ) {
            continue;
        }
        if seen.insert(t.clone()) {
            out.push(t);
        }
        if out.len() >= 12 {
            break;
        }
    }
    out
}

/// Innermost enclosing function and class names at a cursor position. Either
/// may be `None` if the cursor sits at top level.
pub struct Enclosing {
    pub function: Option<String>,
    pub class: Option<String>,
}

/// Walk to the cursor byte offset and find innermost function and class ancestors.
pub fn enclosing_at(tree: &Tree, text: &str, byte_offset: usize) -> Enclosing {
    let root = tree.root_node();
    // A cursor at "end of node" is at the boundary; query one byte earlier so we land
    // inside the last actual token (safer for incomplete files where ERROR truncation
    // refuses the exact end byte).
    let probe = byte_offset.saturating_sub(1).min(text.len());
    let mut node: Option<Node> = root.descendant_for_byte_range(probe, probe).or(Some(root));
    let mut function = None;
    let mut class = None;
    while let Some(n) = node {
        match n.kind() {
            // Function shapes across grammars. tree-sitter-python says
            // "function_definition"; JS/TS say "function_declaration"/"arrow_function";
            // Rust says "function_item"; Go says "function_declaration"; Java/C#
            // method-bodies say "method_definition"/"method_declaration". We list
            // them all — missing one means the encloser won't match a same-file
            // symbol and it leaks into the repo-context block (regurgitation bait).
            "function_declaration"
            | "function_definition"
            | "function_item"
            | "function_expression"
            | "arrow_function"
            | "method_definition"
            | "method_declaration"
            | "generator_function_declaration" => {
                if function.is_none() {
                    function = name_of(&n, text);
                }
            }
            "class_declaration"
            | "class_definition"      // python
            | "interface_declaration" // ts / java / c#
            | "struct_item"           // rust
            | "enum_item"             // rust
            | "trait_item"            // rust
            | "enum_declaration"      // java / ts
            | "struct_specifier"      // c / c++
            | "class_specifier"       // c++
            | "struct_declaration"    // c# -> struct
            => {
                if class.is_none() {
                    class = name_of(&n, text);
                }
            }
            _ => {}
        }
        node = n.parent();
    }
    Enclosing { function, class }
}

/// Read the `name` field of a function/class node with source text.
fn name_of(node: &Node, text: &str) -> Option<String> {
    // Methods store the name under "name" field too (property_identifier).
    node.child_by_field_name("name")
        .and_then(|n| n.utf8_text(text.as_bytes()).ok())
        .map(|s| s.to_string())
}


/// Build the repository_context string for the HTTP payload.
pub async fn build_repository_context(
    index: &Index,
    current_file: &str,
    tree: &Tree,
    text: &str,
    byte_offset: usize,
) -> String {
    let query = identifiers_near_cursor(text, byte_offset, 3);
    build_repository_context_with_query(index, current_file, tree, text, byte_offset, &query).await
}

/// `build_repository_context` with an explicit query override — tests and
/// callers that already extracted identifiers can pass them straight through.
pub async fn build_repository_context_with_query(
    index: &Index,
    current_file: &str,
    tree: &Tree,
    text: &str,
    byte_offset: usize,
    query_terms: &[String],
) -> String {
    let enclosing = enclosing_at(tree, text, byte_offset);
    let current_path = PathBuf::from(current_file);

    let mut lines: Vec<String> = Vec::new();

    // (a) same-file symbols, minus enclosing function/class name AND any name
    // already visible in the last few lines before the cursor (the file is
    // right there in the prompt; repeating the symbol in "Related symbols" is
    // regurgitation bait — the model learns to echo it).
    let in_file = index.symbols_for(&current_path).await;
    let prefix_tail: String = {
        // A few hundred bytes back from the cursor is enough — we're trying to
        // catch freshly-typed `def helper():` / `fn helper(` / `function helper(`.
        let tail_start = byte_offset.saturating_sub(400);
        text[tail_start..byte_offset.min(text.len())].to_owned()
    };
    for s in in_file.iter() {
        if enclosing.function.as_deref() == Some(s.name.as_str())
            || enclosing.class.as_deref() == Some(s.name.as_str())
        {
            continue;
        }
        if prefix_tail.contains(s.name.as_str()) {
            continue;
        }
        lines.push(fmt_symbol(s, current_file));
    }

    // (b) symbols from files that this file imports. Read from the index — its
    // per-language extractor already handles TS `import`, Python `import/from`,
    // Rust `use`, Go `import`, Java `import`, C/Cpp `#include`, C# `using`.
    let specs = index.imports_of(&current_path).await;
    let mut imported_files: std::collections::HashSet<String> = std::collections::HashSet::new();
    for spec in specs.iter() {
        if let Some(resolved) = resolve_import(index, current_file, spec).await {
            let symbols = index.symbols_for(&resolved).await;
            let rel_str = resolved.to_string_lossy().replace('\\', "/");
            imported_files.insert(rel_str.clone());
            for s in symbols.iter() {
                lines.push(fmt_symbol(s, &rel_str));
            }
        }
    }

    // Dedupe (same name+file may appear via multiple imports), preserve order.
    let mut seen = std::collections::HashSet::new();
    lines.retain(|l| seen.insert(l.clone()));

    // Cap at REPO_CTX_CAP bytes on line boundary.
    let mut out = String::new();
    for line in lines {
        let next_len = out.len() + line.len() + 1;
        if next_len > REPO_CTX_CAP {
            break;
        }
        out.push_str(&line);
        out.push('\n');
    }

    // (c) BM25-ranked excerpts from the rest of the repo. Query comes from the
    // caller's `query_terms` (cursor-window identifiers in production).
    if !query_terms.is_empty() {
        let terms: Vec<&str> = query_terms.iter().map(|s| s.as_str()).collect();
        for (path, _score) in index.bm25_rank(&terms).await.into_iter().take(3) {
            // Skip the current file and anything already surfaced via imports.
            let p_str = path.to_string_lossy().replace('\\', "/");
            if p_str == current_file {
                continue;
            }
            if imported_files.contains(&p_str) {
                continue;
            }
            let Some(body) = index.text_for(&path).await else {
                continue;
            };
            // Drop the whole body in (trimmed to what fits). ponytail: smarter
            // excerpting (top-scoring lines only) if prompt noise shows up.
            let header = format!("// {p_str}\n");
            let remaining = REPO_CTX_CAP.saturating_sub(out.len() + header.len() + 1);
            if remaining < 100 {
                break;
            }
            let take: String = body.chars().take(remaining).collect();
            out.push_str(&header);
            out.push_str(take.trim_end());
            out.push('\n');
        }
    }

    out
}
