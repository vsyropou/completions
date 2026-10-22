//! Per-language `import` specifiers → repo-relative path resolution.
//! Reached through [`resolve_import`]/[`resolve_import_for`] from `context`.

use crate::index::Index;
use std::path::{Path, PathBuf};

/// True if the index has content for `p` (any file indexed via `set_text`).
/// Used as the "does the file exist in the workspace" probe for import resolution.
pub(super) async fn indexed_exists(index: &Index, p: &Path) -> bool {
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
pub(super) async fn resolve_ts_import(index: &Index, current_file: &str, spec: &str) -> Option<PathBuf> {
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
/// `extract_c_cpp` strips `<...>`/`"..."` already, so we can't tell the two
/// apart here; we attempt resolution only when the import looks like a file
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
pub(super) fn normalize(p: &Path) -> PathBuf {
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
