//! `.github/relay-sources` names every file the relay image is built from, and
//! CI redeploys the relay only when one of them moved (`changed-tiers.sh`). A
//! bridge-crate module the relay starts to reach without being listed would
//! ship in the next relay image but never trigger one, so this derives the set
//! from the code: the relay bin, the crate root, and every module reachable
//! from them through `crate::`, `super::` and `build_bridge::` paths, test-only
//! items aside. The list's Rust sources must be exactly that set.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::Visit;

const RELAY_BIN: &str = "src/bin/relay.rs";

/// The repo-relative paths (`bridge/src/...`) the list names under the crate's
/// sources, and the rest of the list.
fn listed() -> (BTreeSet<String>, BTreeSet<String>) {
    let list = bridge_dir().join("../.github/relay-sources");
    let text = fs::read_to_string(&list).expect("read .github/relay-sources");
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .partition(|path| path.starts_with("bridge/src/"))
}

fn bridge_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn is_test_gated(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("cfg")
            && attr
                .parse_args::<syn::Ident>()
                .is_ok_and(|ident| ident == "test")
    })
}

/// The module path a source file defines, from `src/`: `src/transport.rs` is
/// `[transport]`, `src/a/b.rs` and `src/a/b/mod.rs` are `[a, b]`, and the
/// crate root and the bin are `[]`.
fn module_path_of(file: &Path) -> Vec<String> {
    let relative = file
        .strip_prefix(bridge_dir().join("src"))
        .expect("a file under src/");
    if relative == Path::new("lib.rs") || relative.starts_with("bin") {
        return Vec::new();
    }
    let mut segments: Vec<String> = relative
        .with_extension("")
        .iter()
        .map(|part| part.to_string_lossy().into_owned())
        .collect();
    if segments.last().is_some_and(|last| last == "mod") {
        segments.pop();
    }
    segments
}

/// The files that make up a top-level module: `src/<name>.rs` or
/// `src/<name>/mod.rs`, and everything under `src/<name>/`. None when the name
/// is not a module file (an item defined in the crate root).
fn files_of_module(name: &str) -> Option<Vec<PathBuf>> {
    let src = bridge_dir().join("src");
    let mut files = Vec::new();
    let flat = src.join(format!("{name}.rs"));
    if flat.is_file() {
        files.push(flat);
    }
    let dir = src.join(name);
    if dir.is_dir() {
        collect_rust_files(&dir, &mut files);
    }
    (!files.is_empty()).then_some(files)
}

fn collect_rust_files(dir: &Path, into: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read module dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_rust_files(&path, into);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            into.push(path);
        }
    }
}

/// Collects the top-level crate modules one file names, resolving `super::`
/// against the file's own module path.
struct CrateReferences<'a> {
    here: &'a [String],
    modules: BTreeSet<String>,
}

impl CrateReferences<'_> {
    /// `segments` is a path as written; record the top-level module it lands in
    /// when it starts at the crate.
    fn record(&mut self, segments: &[String]) {
        let Some(first) = segments.first() else {
            return;
        };
        let resolved: Vec<String> = match first.as_str() {
            "crate" | "build_bridge" => segments[1..].to_vec(),
            "super" => {
                let ups = segments.iter().take_while(|s| *s == "super").count();
                let Some(keep) = self.here.len().checked_sub(ups) else {
                    panic!("`super` past the crate root in {segments:?}");
                };
                let mut path = self.here[..keep].to_vec();
                path.extend_from_slice(&segments[ups..]);
                path
            }
            "self" => {
                let mut path = self.here.to_vec();
                path.extend_from_slice(&segments[1..]);
                path
            }
            _ => return,
        };
        if let Some(top) = resolved.first() {
            self.modules.insert(top.clone());
        }
    }

    fn record_use_tree(&mut self, prefix: &mut Vec<String>, tree: &syn::UseTree) {
        match tree {
            syn::UseTree::Path(path) => {
                prefix.push(path.ident.to_string());
                self.record_use_tree(prefix, &path.tree);
                prefix.pop();
            }
            syn::UseTree::Name(name) => {
                prefix.push(name.ident.to_string());
                self.record(prefix);
                prefix.pop();
            }
            syn::UseTree::Rename(rename) => {
                prefix.push(rename.ident.to_string());
                self.record(prefix);
                prefix.pop();
            }
            syn::UseTree::Glob(_) => self.record(prefix),
            syn::UseTree::Group(group) => {
                for item in &group.items {
                    self.record_use_tree(prefix, item);
                }
            }
        }
    }
}

impl<'ast> Visit<'ast> for CrateReferences<'_> {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        let attrs: &[syn::Attribute] = match item {
            syn::Item::Const(i) => &i.attrs,
            syn::Item::Enum(i) => &i.attrs,
            syn::Item::Fn(i) => &i.attrs,
            syn::Item::Impl(i) => &i.attrs,
            syn::Item::Mod(i) => &i.attrs,
            syn::Item::Static(i) => &i.attrs,
            syn::Item::Struct(i) => &i.attrs,
            syn::Item::Trait(i) => &i.attrs,
            syn::Item::Type(i) => &i.attrs,
            syn::Item::Use(i) => &i.attrs,
            _ => &[],
        };
        if !is_test_gated(attrs) {
            syn::visit::visit_item(self, item);
        }
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        self.record_use_tree(&mut Vec::new(), &item.tree);
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        self.record(&segments);
        syn::visit::visit_path(self, path);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        // A file the relay's code pulls in at compile time is a source of the
        // image that no module walk would find.
        let name = mac.path.segments.last().map(|s| s.ident.to_string());
        assert!(
            !matches!(
                name.as_deref(),
                Some("include" | "include_str" | "include_bytes")
            ),
            "the relay's sources include a file by macro; list it in .github/relay-sources \
             and teach this test to find it"
        );
        syn::visit::visit_macro(self, mac);
    }
}

/// Every Rust source the relay bin is compiled from, as `bridge/src/...` paths.
fn relay_closure() -> BTreeSet<String> {
    let bin = bridge_dir().join(RELAY_BIN);
    let lib = bridge_dir().join("src/lib.rs");
    let mut files: BTreeSet<PathBuf> = BTreeSet::new();
    let mut seen_modules: BTreeSet<String> = BTreeSet::new();
    let mut pending = vec![bin];
    // Modules the bin declares for itself (`mod x;` in relay.rs).
    let bin_modules = bridge_dir().join("src/bin/relay");
    if bin_modules.is_dir() {
        collect_rust_files(&bin_modules, &mut pending);
    }
    while let Some(file) = pending.pop() {
        if !files.insert(file.clone()) {
            continue;
        }
        let source = fs::read_to_string(&file).expect("read source");
        let syntax = syn::parse_file(&source).expect("a file the compiler accepts");
        let here = module_path_of(&file);
        let mut references = CrateReferences {
            here: &here,
            modules: BTreeSet::new(),
        };
        references.visit_file(&syntax);
        // Anything reached in the library is compiled under the crate root.
        if !references.modules.is_empty() {
            pending.push(lib.clone());
        }
        for module in references.modules {
            if !seen_modules.insert(module.clone()) {
                continue;
            }
            if let Some(module_files) = files_of_module(&module) {
                pending.extend(module_files);
            }
        }
    }
    files
        .iter()
        .map(|file| {
            let relative = file.strip_prefix(bridge_dir()).expect("under bridge/");
            format!("bridge/{}", relative.to_string_lossy())
        })
        .collect()
}

#[test]
fn relay_sources_lists_exactly_the_modules_the_relay_is_built_from() {
    let (listed_rust, _) = listed();
    let derived = relay_closure();
    let unlisted: Vec<_> = derived.difference(&listed_rust).collect();
    let stale: Vec<_> = listed_rust.difference(&derived).collect();
    assert!(
        unlisted.is_empty() && stale.is_empty(),
        "the relay is built from {derived:?}. Add to .github/relay-sources (or CI will \
         not redeploy the relay when they change): {unlisted:?}. Remove from it (or \
         bridge-only changes to them will redeploy the relay): {stale:?}"
    );
}

#[test]
fn relay_sources_lists_what_the_image_is_built_with() {
    let (_, listed_other) = listed();
    for required in [
        "bridge/Cargo.toml",
        "bridge/Cargo.lock",
        "bridge/Containerfile",
        "bridge/.dockerignore",
        "deploy/k8s/relay.yaml",
    ] {
        assert!(
            listed_other.contains(required),
            "{required} is missing from .github/relay-sources"
        );
    }
}

/// The walk itself follows a `crate::` path out of a module, so the guard is
/// not passing because it never leaves the bin.
#[test]
fn the_walk_follows_the_relay_into_the_library() {
    let derived = relay_closure();
    assert!(derived.contains("bridge/src/relay_server.rs"));
    assert!(
        derived.contains("bridge/src/transport.rs"),
        "relay_server reaches transport through `use crate::transport`"
    );
}
