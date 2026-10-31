//! `bridge/Containerfile` builds the crate with `bridge/` as its context (at
//! `/src` in the build stage), so a file the release build reads from outside
//! the crate — `include_str!("../../../../fixtures/…")` — is not in the image
//! unless the build stage copies it in from a named build context, to the path
//! the include resolves to. #913 added one without that and broke every image
//! build. This derives the reads from the code: every `include_str!` /
//! `include_bytes!` in a module the release binaries compile (cfg(test) and the
//! `testing` feature aside) whose path leaves `bridge/`. Each must be copied in
//! place by the build stage, and every workflow step and compose service that
//! builds the Containerfile must name the contexts those copies come from, at
//! the repo directory the copy assumes.
//!
//! The walk fails closed: a `mod` it cannot find a file for, a file module
//! declared inside an inline module, and an include whose argument is not a
//! string literal are errors, never skipped.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use syn::buffer::{Cursor, TokenBuffer};
use syn::visit::Visit;

fn bridge_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_dir() -> PathBuf {
    normalize(&bridge_dir().join(".."))
}

/// Lexically resolve `.` and `..`, without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// A cfg predicate under the image's release build on Linux: `Some(false)`
/// only when it is certainly off there, `None` when this cannot tell.
fn cfg_value(meta: &syn::Meta) -> Option<bool> {
    match meta {
        syn::Meta::Path(path) if path.is_ident("test") => Some(false),
        syn::Meta::Path(path) if path.is_ident("unix") => Some(true),
        syn::Meta::Path(path) if path.is_ident("windows") => Some(false),
        syn::Meta::NameValue(nv) => {
            let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(value),
                ..
            }) = &nv.value
            else {
                return None;
            };
            if nv.path.is_ident("feature") {
                // The image builds with no features (bridge/Cargo.toml has no default).
                Some(false)
            } else if nv.path.is_ident("target_os") {
                Some(value.value() == "linux")
            } else {
                None
            }
        }
        syn::Meta::List(list) => {
            let args = list
                .parse_args_with(
                    syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
                )
                .ok()?;
            let values: Vec<_> = args.iter().map(cfg_value).collect();
            if list.path.is_ident("not") {
                values.first().copied().flatten().map(|v| !v)
            } else if list.path.is_ident("all") {
                if values.contains(&Some(false)) {
                    Some(false)
                } else if values.iter().all(|v| *v == Some(true)) {
                    Some(true)
                } else {
                    None
                }
            } else if list.path.is_ident("any") {
                if values.contains(&Some(true)) {
                    Some(true)
                } else if values.iter().all(|v| *v == Some(false)) {
                    Some(false)
                } else {
                    None
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Off in the image's build: some `#[cfg(…)]` on it is certainly false.
fn compiled_out(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("cfg")
            && attr
                .parse_args::<syn::Meta>()
                .is_ok_and(|meta| cfg_value(&meta) == Some(false))
    })
}

fn path_attr(attrs: &[syn::Attribute]) -> Option<String> {
    attrs.iter().find_map(|attr| match &attr.meta {
        syn::Meta::NameValue(nv) if nv.path.is_ident("path") => match &nv.value {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(value),
                ..
            }) => Some(value.value()),
            _ => None,
        },
        _ => None,
    })
}

/// One source file's includes and its `mod x;` children, test-only items aside.
struct FileScan<'a> {
    file: &'a Path,
    /// Where the next `mod x;` finds `x.rs`: the file's child directory, then
    /// one level per inline module around it.
    dirs: Vec<PathBuf>,
    includes: Vec<Result<PathBuf, String>>,
    children: Vec<(PathBuf, PathBuf)>,
}

impl FileScan<'_> {
    fn include(&mut self, name: &str, literal: Option<String>) {
        let dir = self.file.parent().unwrap();
        self.includes.push(match literal {
            Some(path) => Ok(normalize(&dir.join(path))),
            None => Err(format!(
                "{}: {name}! without a string literal this can follow",
                self.file.display()
            )),
        });
    }

    /// Every `include_str!(…)` / `include_bytes!(…)` inside another macro's
    /// tokens, nested groups included.
    fn includes_in_tokens(&mut self, mut cursor: Cursor) {
        while !cursor.eof() {
            if let Some((ident, next)) = cursor.ident() {
                let name = ident.to_string();
                let bang = next.punct().filter(|(p, _)| p.as_char() == '!');
                match bang.and_then(|(_, after)| after.any_group()) {
                    Some((inside, _, _, after)) if INCLUDES.contains(&name.as_str()) => {
                        self.include(&name, include_target(inside));
                        cursor = after;
                    }
                    _ => cursor = next,
                }
            } else if let Some((inside, _, _, next)) = cursor.any_group() {
                self.includes_in_tokens(inside);
                cursor = next;
            } else if let Some((_, next)) = cursor.token_tree() {
                cursor = next;
            } else {
                break;
            }
        }
    }

    fn child_file(&self, module: &syn::ItemMod) -> Result<(PathBuf, PathBuf), String> {
        let name = module.ident.to_string();
        let dir = self.dirs.last().unwrap();
        if let Some(path) = path_attr(&module.attrs) {
            if self.dirs.len() > 1 {
                return Err(format!(
                    "{}: `#[path] mod {name};` inside an inline module",
                    self.file.display()
                ));
            }
            let child = normalize(&self.file.parent().unwrap().join(path));
            let grandchildren = child.with_extension("");
            return Ok((child, grandchildren));
        }
        let flat = dir.join(format!("{name}.rs"));
        let nested = dir.join(&name).join("mod.rs");
        match (flat.is_file(), nested.is_file()) {
            (true, _) => Ok((flat, dir.join(&name))),
            (false, true) => Ok((nested, dir.join(&name))),
            _ => Err(format!(
                "{}: no file for `mod {name};`",
                self.file.display()
            )),
        }
    }
}

const INCLUDES: [&str; 2] = ["include_str", "include_bytes"];

fn string_literal(cursor: Cursor) -> Option<(String, Cursor)> {
    let (literal, rest) = cursor.literal()?;
    match syn::Lit::new(literal) {
        syn::Lit::Str(value) => Some((value.value(), rest)),
        _ => None,
    }
}

/// What an include's arguments name: the path of a lone string literal, or for
/// `concat!("dir/", $file)` (a template macro's) the directory its literal
/// prefix names, which a copy must then cover whole.
fn include_target(arguments: Cursor) -> Option<String> {
    if let Some((path, rest)) = string_literal(arguments) {
        return rest.eof().then_some(path);
    }
    let (ident, rest) = arguments.ident()?;
    let (bang, rest) = rest.punct()?;
    if ident != "concat" || bang.as_char() != '!' {
        return None;
    }
    let (inside, _, _, rest) = rest.any_group()?;
    let (prefix, _) = string_literal(inside)?;
    let directory = prefix.rfind('/').map_or("", |at| &prefix[..at]);
    rest.eof().then(|| directory.to_string())
}

impl<'ast> Visit<'ast> for FileScan<'_> {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        if !compiled_out(item_attrs(item)) {
            syn::visit::visit_item(self, item);
        }
    }

    fn visit_impl_item(&mut self, item: &'ast syn::ImplItem) {
        let attrs: &[syn::Attribute] = match item {
            syn::ImplItem::Const(i) => &i.attrs,
            syn::ImplItem::Fn(i) => &i.attrs,
            syn::ImplItem::Type(i) => &i.attrs,
            syn::ImplItem::Macro(i) => &i.attrs,
            _ => &[],
        };
        if !compiled_out(attrs) {
            syn::visit::visit_impl_item(self, item);
        }
    }

    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        let Some((_, content)) = &module.content else {
            match self.child_file(module) {
                Ok(child) => self.children.push(child),
                Err(error) => self.includes.push(Err(error)),
            }
            return;
        };
        let inner = self.dirs.last().unwrap().join(module.ident.to_string());
        self.dirs.push(inner);
        for item in content {
            self.visit_item(item);
        }
        self.dirs.pop();
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let name = mac
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        if INCLUDES.contains(&name.as_str()) {
            let buffer = TokenBuffer::new2(mac.tokens.clone());
            self.include(&name, include_target(buffer.begin()));
        } else {
            let buffer = TokenBuffer::new2(mac.tokens.clone());
            self.includes_in_tokens(buffer.begin());
        }
        syn::visit::visit_macro(self, mac);
    }
}

fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Const(i) => &i.attrs,
        syn::Item::Enum(i) => &i.attrs,
        syn::Item::Fn(i) => &i.attrs,
        syn::Item::Impl(i) => &i.attrs,
        syn::Item::Macro(i) => &i.attrs,
        syn::Item::Mod(i) => &i.attrs,
        syn::Item::Static(i) => &i.attrs,
        syn::Item::Struct(i) => &i.attrs,
        syn::Item::Trait(i) => &i.attrs,
        syn::Item::Type(i) => &i.attrs,
        syn::Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

#[derive(Default)]
struct Walk {
    seen: BTreeSet<PathBuf>,
    includes: Vec<Result<PathBuf, String>>,
}

impl Walk {
    /// `file`, whose `mod x;` children live under `child_dir`, and theirs.
    fn file(&mut self, file: &Path, child_dir: &Path) {
        if !self.seen.insert(file.to_path_buf()) {
            return;
        }
        let source = fs::read_to_string(file).unwrap_or_else(|e| panic!("read {file:?}: {e}"));
        let parsed = syn::parse_file(&source).unwrap_or_else(|e| panic!("parse {file:?}: {e}"));
        if compiled_out(&parsed.attrs) {
            return;
        }
        let mut scan = FileScan {
            file,
            dirs: vec![child_dir.to_path_buf()],
            includes: Vec::new(),
            children: Vec::new(),
        };
        scan.visit_file(&parsed);
        self.includes.append(&mut scan.includes);
        for (child, grandchildren) in scan.children {
            self.file(&child, &grandchildren);
        }
    }
}

/// Repo-relative paths outside `bridge/` that the release binaries include.
fn release_includes_outside_crate() -> BTreeSet<String> {
    let src = bridge_dir().join("src");
    let mut walk = Walk::default();
    walk.file(&src.join("lib.rs"), &src);
    walk.file(&src.join("main.rs"), &src);
    walk.file(&src.join("bin/relay.rs"), &src.join("bin"));
    assert!(
        walk.seen.len() > 50,
        "the walk reached {} files",
        walk.seen.len()
    );
    let (found, errors): (Vec<_>, Vec<_>) = walk.includes.into_iter().partition(Result::is_ok);
    let errors: Vec<_> = errors.into_iter().map(Result::unwrap_err).collect();
    assert!(errors.is_empty(), "the walk cannot answer: {errors:#?}");
    let bridge = normalize(&bridge_dir());
    let repo = repo_dir();
    found
        .into_iter()
        .map(Result::unwrap)
        .filter(|path| !path.starts_with(&bridge))
        .map(|path| {
            path.strip_prefix(&repo)
                .unwrap_or_else(|_| panic!("{path:?} is outside the repo"))
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

/// The build stage's `COPY --from=<context>` lines from a named context (not
/// a stage), as (context, source, destination).
fn build_stage_context_copies() -> Vec<(String, String, String)> {
    let containerfile = fs::read_to_string(bridge_dir().join("Containerfile")).unwrap();
    let stages: BTreeSet<String> = containerfile
        .lines()
        .filter_map(|line| {
            let words: Vec<_> = line.split_whitespace().collect();
            (words.first() == Some(&"FROM")
                && words.get(2).is_some_and(|w| w.eq_ignore_ascii_case("AS")))
            .then(|| words[3].to_string())
        })
        .collect();
    let build = containerfile
        .split("\nFROM ")
        .find(|stage| {
            stage
                .lines()
                .next()
                .is_some_and(|l| l.trim_end().ends_with("AS build"))
        })
        .expect("bridge/Containerfile has a build stage");
    let mut workdir = PathBuf::from("/");
    let mut copies = Vec::new();
    for line in build.lines() {
        let words: Vec<_> = line.split_whitespace().collect();
        match words.first() {
            Some(&"WORKDIR") => workdir = normalize(&workdir.join(words[1])),
            Some(&"COPY") => {
                let Some(from) = words.iter().find_map(|w| w.strip_prefix("--from=")) else {
                    continue;
                };
                if stages.contains(from) || from.contains(':') || from.contains('/') {
                    continue;
                }
                let args: Vec<_> = words[1..].iter().filter(|w| !w.starts_with("--")).collect();
                let destination = normalize(&workdir.join(args.last().unwrap()));
                for source in &args[..args.len() - 1] {
                    copies.push((
                        from.to_string(),
                        source.trim_end_matches('/').to_string(),
                        destination.to_string_lossy().into_owned(),
                    ));
                }
            }
            _ => {}
        }
    }
    copies
}

/// The lines after `start` indented deeper than it: its YAML block.
fn block_after<'a>(lines: &[&'a str], start: usize) -> Vec<&'a str> {
    let indent = |line: &str| line.len() - line.trim_start().len();
    let base = indent(lines[start]);
    lines[start + 1..]
        .iter()
        .take_while(|line| line.trim().is_empty() || indent(line) > base)
        .copied()
        .collect()
}

/// Every place an image is built from bridge/Containerfile, with the named
/// contexts it passes (name → repo-relative directory).
fn containerfile_builders() -> Vec<(String, BTreeMap<String, String>)> {
    let repo = repo_dir();
    let mut builders = Vec::new();
    let mut files: Vec<PathBuf> = fs::read_dir(repo.join(".github/workflows"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .chain(
            fs::read_dir(repo.join("deploy"))
                .unwrap()
                .map(|entry| entry.unwrap().path()),
        )
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext == "yml" || ext == "yaml")
        })
        .collect();
    files.sort();
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let relative = file
            .strip_prefix(&repo)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let base = file.parent().unwrap();
        let is_workflow = relative.starts_with(".github/");
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            // A workflow step's `with:` or a compose service's `build:`.
            if trimmed != "with:" && trimmed != "build:" {
                continue;
            }
            let block = block_after(&lines, i);
            let value = |key: &str| {
                block.iter().find_map(|l| {
                    l.trim()
                        .strip_prefix(key)
                        .and_then(|rest| rest.strip_prefix(':'))
                        .map(|v| v.trim().to_string())
                })
            };
            let builds_bridge = if is_workflow {
                value("file").as_deref() == Some("bridge/Containerfile")
            } else {
                let context = value("context").unwrap_or_default();
                let dockerfile = value("dockerfile").unwrap_or_else(|| "Containerfile".into());
                normalize(&base.join(&context).join(&dockerfile))
                    == repo.join("bridge/Containerfile")
            };
            if !builds_bridge {
                continue;
            }
            let key = if is_workflow {
                "build-contexts:"
            } else {
                "additional_contexts:"
            };
            let mut contexts = BTreeMap::new();
            if let Some(at) = block.iter().position(|l| l.trim().starts_with(key)) {
                for entry in block_after(&block, at) {
                    let entry = entry.trim().trim_start_matches("- ");
                    let (name, dir) = match entry.split_once('=') {
                        Some(pair) => pair,
                        None => match entry.split_once(':') {
                            Some(pair) => pair,
                            None => continue,
                        },
                    };
                    let dir = normalize(&base_dir(&repo, base, is_workflow).join(dir.trim()));
                    contexts.insert(
                        name.trim().to_string(),
                        dir.strip_prefix(&repo)
                            .unwrap()
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
            builders.push((format!("{relative}:{}", i + 1), contexts));
        }
    }
    builders
}

/// Workflow paths are relative to the checkout; compose paths to the file.
fn base_dir(repo: &Path, file_dir: &Path, is_workflow: bool) -> PathBuf {
    if is_workflow {
        repo.to_path_buf()
    } else {
        file_dir.to_path_buf()
    }
}

#[test]
fn the_walk_finds_the_http_resource_manifest() {
    // The read #913 added, so the walk is known to see it.
    assert!(
        release_includes_outside_crate().contains("fixtures/api/http/resource-manifest.json"),
        "{:?}",
        release_includes_outside_crate()
    );
}

#[test]
fn every_read_outside_the_crate_is_copied_in_place_by_the_build_stage() {
    // The crate sits at /src, so `<repo>/x` is read at `/x`.
    let copies = build_stage_context_copies();
    let builders = containerfile_builders();
    let contexts = &builders
        .first()
        .expect("something builds bridge/Containerfile")
        .1;
    let missing: Vec<_> = release_includes_outside_crate()
        .into_iter()
        .filter(|read| {
            !copies.iter().any(|(context, source, destination)| {
                contexts.get(context).is_some_and(|dir| {
                    let repo_source = normalize(&Path::new(dir).join(source));
                    Path::new(destination) == Path::new("/").join(&repo_source)
                        && Path::new(read).starts_with(&repo_source)
                })
            })
        })
        .collect();
    assert!(
        missing.is_empty(),
        "the release build reads {missing:?} from outside bridge/: add a `COPY --from=<context>` \
         to bridge/Containerfile's build stage that lands it at /<repo path>, and the context \
         to every build of it. Copies now: {copies:?}; contexts: {contexts:?}"
    );
}

#[test]
fn every_build_of_the_containerfile_passes_the_same_contexts() {
    let builders = containerfile_builders();
    let names: BTreeSet<_> = builders
        .iter()
        .map(|(at, _)| at.split(':').next().unwrap())
        .collect();
    for expected in [
        ".github/workflows/ci.yml",
        ".github/workflows/dev-deploy.yml",
        "deploy/compose.real.yml",
        "deploy/compose.two-bridges.yml",
    ] {
        assert!(
            names.contains(expected),
            "found no build of bridge/Containerfile in {expected}: {builders:?}"
        );
    }
    let needed: BTreeSet<_> = build_stage_context_copies()
        .into_iter()
        .map(|(context, ..)| context)
        .collect();
    let first = &builders[0].1;
    for (at, contexts) in &builders {
        let named: BTreeSet<_> = contexts.keys().cloned().collect();
        assert!(
            needed.is_subset(&named) && contexts == first,
            "{at} builds bridge/Containerfile with contexts {contexts:?}; the build stage copies \
             from {needed:?} and {} passes {first:?}",
            builders[0].0
        );
    }
}
