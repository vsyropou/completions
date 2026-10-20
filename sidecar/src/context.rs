//! Assemble the `repository_context` string sent to Phoenix. This is where
//! *context selection* lives: which symbols/imports make it into the prompt.
//!
//! To *change* what gets picked, edit [`build_repository_context`]. To change
//! the *ranking* heuristic (which files are even considered), see
//! `crate::index::Index` and the way we resolve `import ... from "./y"` here.

use crate::index::{Index, Symbol};
use std::path::{Path, PathBuf};
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
/// Falls back to linewise symmetric expansion (Zed's `expand_symmetric_from_cursor`:
/// alternate down-then-up one line at a time, charge max(1, bytes/3) tokens per line).
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
            "function_declaration"
            | "function_expression"
            | "arrow_function"
            | "method_definition"
            | "generator_function_declaration" => {
                if function.is_none() {
                    function = name_of(&n, text);
                }
            }
            "class_declaration" => {
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

/// First N import statements of the document (raw source text lines).
pub fn imports_of(tree: &Tree, text: &str, cap: usize) -> Vec<String> {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut out = Vec::new();
    for child in root.children(&mut cursor) {
        if child.kind() == "import_statement" {
            if out.len() >= cap {
                break;
            }
            if let Ok(t) = child.utf8_text(text.as_bytes()) {
                out.push(t.trim_end_matches(['\n', ';']).to_string() + ";");
            }
        }
    }
    out
}

/// Import specifiers (the "./y" parts) — for symbol lookup.
pub fn import_specifiers(tree: &Tree, text: &str, cap: usize) -> Vec<String> {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut out = Vec::new();
    for child in root.children(&mut cursor) {
        if child.kind() == "import_statement" {
            if out.len() >= cap {
                break;
            }
            let mut cur = child.walk();
            for c in child.children(&mut cur) {
                if c.kind() == "string" {
                    if let Ok(s) = c.utf8_text(text.as_bytes()) {
                        let t = s.trim_matches(&['"', '\''][..]).to_string();
                        out.push(t);
                        break;
                    }
                }
            }
        }
    }
    out
}

/// True if the index has content for `p` (any file indexed via `set_text`).
/// Used as the "does the file exist in the workspace" probe for import resolution.
async fn indexed_exists(index: &Index, p: &Path) -> bool {
    index.text_for(p).await.is_some()
}

/// Resolve an import specifier against the current file path. Picks the
/// per-language resolver based on `current_file`'s extension.
pub async fn resolve_import(index: &Index, current_file: &str, spec: &str) -> Option<PathBuf> {
    let lang = crate::syntax::Lang::from_path(Path::new(current_file));
    resolve_import_for(index, lang, current_file, spec).await
}

/// Per-language dispatch. `lang` is the *importing* file's language.
pub async fn resolve_import_for(
    index: &Index,
    lang: crate::syntax::Lang,
    current_file: &str,
    spec: &str,
) -> Option<PathBuf> {
    use crate::syntax::Lang::*;
    match lang {
        TypeScript | Tsx | JavaScript | Jsx => resolve_ts_import(index, current_file, spec).await,
        Python => resolve_python_import(index, current_file, spec).await,
        Rust => resolve_rust_import(index, current_file, spec).await,
        Go => resolve_go_import(index, current_file, spec).await,
        Java => resolve_java_import(index, current_file, spec).await,
        C | Cpp => resolve_c_include(index, current_file, spec).await,
        // TODO(csharp): `using X.Y;` is namespace-based, no file mapping. Revisit if
        // prompts need cross-file C# context.
        CSharp | Elixir | Json | Yaml | NoParser => None,
    }
}

/// TS/JS: `"./y"`, `"../z"` → `.ts|.tsx|.js|.jsx` siblings or `index.*` child.
async fn resolve_ts_import(index: &Index, current_file: &str, spec: &str) -> Option<PathBuf> {
    if !spec.starts_with('.') {
        return None; // package imports not indexed
    }
    let current_dir = Path::new(current_file).parent().unwrap_or(Path::new(""));
    let base = current_dir.join(spec);
    // Try ./y.ts, ./y.tsx, ./y.js, ./y.jsx, ./y/index.* variants.
    let mut cands: Vec<PathBuf> = Vec::new();
    for ext in ["ts", "tsx", "js", "jsx"] {
        cands.push(PathBuf::from(format!("{}.{}", base.display(), ext)));
    }
    for ext in ["ts", "tsx", "js", "jsx"] {
        cands.push(PathBuf::from(format!("{}/index.{}", base.display(), ext)));
    }
    // Also allow the spec itself when it already carries an extension we index.
    if ["ts", "tsx", "js", "jsx", "mjs", "cjs"]
        .iter()
        .any(|e| spec.ends_with(&format!(".{e}")))
    {
        cands.push(base.clone());
    }
    for cand in cands {
        let p = normalize(&cand);
        if indexed_exists(&index, &p).await {
            return Some(p);
        }
    }
    None
}

/// Python:
///  - "foo.bar" → `foo/bar.py` or `foo/bar/__init__.py`, walking up from importing file.
///  - "foo" (from `from foo import x`) → `foo.py` / `foo/__init__.py`, same walk.
///  - ".foo" / "..foo" (relative) — each `.` walks one dir up from importing file's dir.
pub async fn resolve_python_import(
    index: &Index,
    current_file: &str,
    spec: &str,
) -> Option<PathBuf> {
    let current_dir = Path::new(current_file).parent().unwrap_or(Path::new("")).to_path_buf();
    // Count leading dots (1 = current package, 2 = parent, ...).
    let dots = spec.chars().take_while(|c| *c == '.').count();
    let rest = spec.trim_start_matches('.');
    let mut base = current_dir.clone();
    // "from . import x" → 1 dot == current dir itself.
    for _ in 1..dots {
        if !base.pop() {
            return None;
        }
    }
    let module_path = rest.replace('.', "/");
    // Try module_path.py and module_path/__init__.py under each ancestor of `base`.
    let mut dir = Some(base.as_path());
    while let Some(d) = dir {
        for cand in [
            d.join(format!("{module_path}.py")),
            d.join(&module_path).join("__init__.py"),
        ] {
            let p = normalize(&cand);
            if indexed_exists(&index, &p).await {
                return Some(p);
            }
        }
        dir = d.parent();
    }
    None
}

/// Rust:
///  - `crate::foo::Bar` / `crate::foo` → `foo/Bar.rs` / `foo.rs` / `foo/mod.rs`, walking up
///    (approximates "Cargo project root" without needing `Cargo.toml`).
///  - `super::foo` → walk up one dir per `super::` prefix.
///  - `self::foo` → current dir.
///  - `foo` (no prefix) → external crate; not resolved.
/// Rust:
///  - `crate::foo::Bar` / `crate::foo` → `foo/Bar.rs` / `foo.rs` / `foo/mod.rs`, walking up
///    (approximates "Cargo project root" without needing `Cargo.toml`).
///  - `super::foo` → walk up one dir per `super::` prefix.
///  - `self::foo` → current dir.
///  - `foo` (no prefix) → external crate; not resolved.
pub async fn resolve_rust_import(index: &Index, current_file: &str, spec: &str) -> Option<PathBuf> {
    // Spec may carry trailing braces / names like "crate::foo::{Bar, Baz}" or "crate::foo::Bar".
    // Normalize to the longest path-like prefix.
    let cleaned = spec.split(&['{', '}', ',', ';'][..]).next().unwrap_or(spec).trim();
    // Supers first — count them, then strip.
    let mut rest = cleaned;
    let mut ups = 0usize;
    loop {
        if let Some(r) = rest.strip_prefix("super::") {
            rest = r;
            ups += 1;
        } else {
            break;
        }
    }
    let (base_hint, tail): (Option<PathBuf>, &str) = if let Some(r) = rest.strip_prefix("crate::") {
        (None, r) // walk up from current dir
    } else if let Some(r) = rest.strip_prefix("self::") {
        let d = Path::new(current_file).parent().unwrap_or(Path::new("")).to_path_buf();
        (Some(d), r)
    } else if ups > 0 {
        // Rust module semantics: from `a/b/c.rs` (mod `c`), `super` is the module
        // `b` whose sibling files live in `a/b/`. From `a/b/mod.rs` (mod `b`),
        // `super` is `a` — files in `a/`. Each extra `super` climbs one more dir.
        let d0 = Path::new(current_file).parent().unwrap_or(Path::new("")).to_path_buf();
        let is_mod_rs = Path::new(current_file)
            .file_name()
            .and_then(|f| f.to_str())
            .map(|f| f == "mod.rs")
            .unwrap_or(false);
        let mut dir = if is_mod_rs {
            // mod.rs: super climbs immediately.
            match d0.parent() {
                Some(p) => p.to_path_buf(),
                None => return None,
            }
        } else {
            d0
        };
        // Each extra super (beyond the one that located the base dir) climbs once.
        let extra_climbs = ups.saturating_sub(1);
        for _ in 0..extra_climbs {
            if !dir.pop() {
                return None;
            }
        }
        (Some(dir), rest)
    } else {
        // No crate/self/super prefix → external crate name. Not resolvable.
        return None;
    };
    // `tail` may be dotted nested like "foo::Bar" — first segment is the module dir/file.
    let segments: Vec<&str> = tail.split("::").filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return None;
    }
    // Try under candidate bases: explicit base (self/super), or walking up from current dir.
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(b) = base_hint {
        candidates.push(b);
    } else {
        // walk-up search: every ancestor of the importing file is a candidate crate root.
        let mut d = Path::new(current_file).parent().map(|p| p.to_path_buf());
        while let Some(dir) = d {
            candidates.push(dir.clone());
            d = dir.parent().map(|p| p.to_path_buf());
        }
    }
    // Common shapes: Crate::foo::Bar → foo/Bar.rs? Real rust source files are lowercase
    // module paths. Try `foo/bar.rs` and `foo/bar/mod.rs` where `bar` is the last segment.
    let mut lower_segs: Vec<String> = segments.iter().map(|s| s.to_string()).collect();
    // Lowercase *only* the last segment for file-name purposes (module files are snake).
    if let Some(last) = lower_segs.last_mut() {
        *last = last.to_lowercase();
    }
    let joined_dir = lower_segs.join("/");
    for base in candidates {
        for cand in [
            base.join(format!("{joined_dir}.rs")),
            base.join(&joined_dir).join("mod.rs"),
        ] {
            let p = normalize(&cand);
            if indexed_exists(&index, &p).await {
                return Some(p);
            }
        }
    }
    None
}

/// Go: only relative "./foo" is resolvable. Real Go imports are module paths and
/// can't be mapped without go.mod parsing — skip them.
/// Go: only relative "./foo" is resolvable. Real Go imports are module paths and
/// can't be mapped without go.mod parsing — skip them.
pub async fn resolve_go_import(index: &Index, current_file: &str, spec: &str) -> Option<PathBuf> {
    if !spec.starts_with("./") {
        return None;
    }
    let current_dir = Path::new(current_file).parent().unwrap_or(Path::new(""));
    let cand = current_dir.join(format!("{}.go", &spec[2..]));
    let p = normalize(&cand);
    if indexed_exists(&index, &p).await {
        return Some(p);
    }
    None
}

/// Java: `com.foo.Bar` → `com/foo/Bar.java`, rooted at any ancestor of the
/// importing file or under an ancestor's `src/main/java/`.
/// Java: `com.foo.Bar` → `com/foo/Bar.java`, rooted at any ancestor of the
/// importing file or under an ancestor's `src/main/java/`.
pub async fn resolve_java_import(index: &Index, current_file: &str, spec: &str) -> Option<PathBuf> {
    let path = spec.replace('.', "/");
    let file = format!("{path}.java");
    let mut dir = Path::new(current_file).parent().map(|p| p.to_path_buf());
    while let Some(d) = dir {
        for cand in [d.join(&file), d.join("src/main/java").join(&file)] {
            let p = normalize(&cand);
            if indexed_exists(&index, &p).await {
                return Some(p);
            }
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    None
}

/// C/Cpp: quoted `#include "foo.h"` resolves adjacent to the importer, then in
/// ancestor dirs of the project root. Angle includes are system headers — skip.
/// Note: `extract_c_cpp` strips `<...>`/`"..."` already, so we can't tell the
/// two apart here; we attempt resolution only when the import looks like a file
/// (has a path-ish or `.h`-ish shape). Angle includes will simply fail lookup.
/// C/Cpp: quoted `#include "foo.h"` resolves adjacent to the importer, then in
/// ancestor dirs of the project root. Angle includes are system headers — skip.
/// Note: `extract_c_cpp` strips `<...>`/`"..."` already, so we can't tell the
/// two apart here; we attempt resolution only when the import looks like a file
/// (has a path-ish or `.h`-ish shape). Angle includes will simply fail lookup.
pub async fn resolve_c_include(index: &Index, current_file: &str, spec: &str) -> Option<PathBuf> {
    // Cheap system-header gate: no path separator and no common C/C++ header ext.
    let looks_system = !spec.contains('/')
        && !spec.ends_with(".h")
        && !spec.ends_with(".hpp")
        && !spec.ends_with(".hh")
        && !spec.ends_with(".hxx")
        && !spec.ends_with(".c")
        && !spec.ends_with(".cc")
        && !spec.ends_with(".cpp")
        && !spec.ends_with(".cxx");
    if looks_system {
        return None;
    }
    let current_dir = Path::new(current_file).parent().unwrap_or(Path::new("")).to_path_buf();
    let mut dir = Some(current_dir.as_path());
    while let Some(d) = dir {
        let direct = normalize(&d.join(spec));
        if indexed_exists(&index, &direct).await {
            return Some(direct);
        }
        // If it's a .h/.hpp, also try the matching implementation TU.
        for ext_swap in [".c", ".cc", ".cpp", ".cxx"] {
            if spec.ends_with(".h") || spec.ends_with(".hpp") || spec.ends_with(".hh") {
                let stem = spec
                    .trim_end_matches(".hpp")
                    .trim_end_matches(".hh")
                    .trim_end_matches(".h");
                let cand = normalize(&d.join(format!("{stem}{ext_swap}")));
                if indexed_exists(&index, &cand).await {
                    return Some(cand);
                }
            }
        }
        dir = d.parent();
    }
    None
}

/// Lexically collapse `.` and `..` segments in `p` (like `Path::normalize`
/// would, but stdlib doesn't have one). Does not touch the filesystem.
fn normalize(p: &Path) -> PathBuf {
    // Resolve "a/./b" and "a/../b" lexically (Path::normalize equivalent via components).
    let mut out = PathBuf::new();
    for comp in p.components() {
        use std::path::Component::*;
        match comp {
            CurDir => {}
            ParentDir => {
                out.pop();
            }
            RootDir => out.push("/"),
            Normal(part) => out.push(part),
            Prefix(prefix) => out.push(prefix.as_os_str()),
        }
    }
    out
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

    // (a) same-file symbols, minus enclosing function/class name
    let in_file = index.symbols_for(&current_path).await;
    for s in in_file.iter() {
        if enclosing.function.as_deref() == Some(s.name.as_str())
            || enclosing.class.as_deref() == Some(s.name.as_str())
        {
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

/// Format one index symbol as a context line: `kind name (file:line_1based)`.
/// Kind wording depends on `lang` (fn/def/func/method) so FIM prompts read
/// naturally to the model regardless of source language.
fn fmt_symbol(s: &Symbol, file: &str) -> String {
    let kind_word = kind_label(s.lang, s.kind);
    // line here is 0-based row; report 1-based
    format!("{} {} ({}:{})", kind_word, s.name, file, s.line + 1)
}

/// Map a generic kind to a per-language label. Single token, no spaces.
fn kind_label(lang: crate::syntax::Lang, kind: &str) -> &'static str {
    use crate::syntax::Lang::*;
    match (lang, kind) {
        (Rust, "function") => "fn",
        (Rust, "module") => "mod",
        (Python, "function") => "def",
        (Go, "function") | (Go, "method") => "func",
        (Java, "method") | (CSharp, "method") => "method",
        (C, "function") | (Cpp, "function") => "fn",
        (_, "function") => "function",
        (_, "method") => "method",
        (_, "class") => "class",
        (_, "interface") => "interface",
        (_, "type") => "type",
        (_, "const") => "const",
        (_, "module") => "module",
        _ => "symbol",
    }
}

