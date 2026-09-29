//! `.github/relay-sources` names every file the relay image is built from, and
//! CI redeploys the relay only when one of them moved (`changed-tiers.sh`). A
//! bridge-crate module the relay starts to reach without being listed would
//! ship in the next relay image but never trigger one, so this derives the set
//! from the code: the relay bin, the crate root, and every module reachable
//! from them through `crate::`, `super::`, `self::` and `build_bridge::` paths
//! — in code and inside macro invocations alike, through lib.rs re-exports, and
//! into any module that implements a type the relay reaches. Test-only items
//! aside. The list's Rust sources must be exactly that set.
//!
//! The walk fails closed: a crate path it cannot resolve to a file, a glob of
//! the crate root, and a file pulled in by `include!`-style macros are errors,
//! never skipped.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use syn::buffer::{Cursor, TokenBuffer};
use syn::visit::Visit;

const RELAY_BIN: &str = "bin/relay.rs";

fn is_test_gated(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("cfg")
            && attr
                .parse_args::<syn::Ident>()
                .is_ok_and(|ident| ident == "test")
    })
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
        syn::Item::Union(i) => &i.attrs,
        syn::Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

fn item_ident(item: &syn::Item) -> Option<&syn::Ident> {
    match item {
        syn::Item::Const(i) => Some(&i.ident),
        syn::Item::Enum(i) => Some(&i.ident),
        syn::Item::Fn(i) => Some(&i.sig.ident),
        syn::Item::Macro(i) => i.ident.as_ref(),
        syn::Item::Static(i) => Some(&i.ident),
        syn::Item::Struct(i) => Some(&i.ident),
        syn::Item::Trait(i) => Some(&i.ident),
        syn::Item::Type(i) => Some(&i.ident),
        syn::Item::Union(i) => Some(&i.ident),
        // An inline module lives in lib.rs; a `mod x;` is found by its file.
        syn::Item::Mod(i) if i.content.is_some() => Some(&i.ident),
        _ => None,
    }
}

fn parse(file: &Path) -> syn::File {
    let source = fs::read_to_string(file).unwrap_or_else(|e| panic!("read {file:?}: {e}"));
    syn::parse_file(&source).unwrap_or_else(|e| panic!("parse {file:?}: {e}"))
}

/// Every `(name, path)` a use tree brings into scope, and its globs as the
/// path they glob (ending in `*`). A `self` leaf names its parent.
fn use_leaves(prefix: &mut Vec<String>, tree: &syn::UseTree, out: &mut Vec<(String, Vec<String>)>) {
    match tree {
        syn::UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            use_leaves(prefix, &path.tree, out);
            prefix.pop();
        }
        syn::UseTree::Name(name) if name.ident == "self" => {
            let parent = prefix.last().cloned().unwrap_or_default();
            out.push((parent, prefix.clone()));
        }
        syn::UseTree::Name(name) => {
            let mut path = prefix.clone();
            path.push(name.ident.to_string());
            out.push((name.ident.to_string(), path));
        }
        syn::UseTree::Rename(rename) => {
            let mut path = prefix.clone();
            path.push(rename.ident.to_string());
            out.push((rename.rename.to_string(), path));
        }
        syn::UseTree::Glob(_) => {
            let mut path = prefix.clone();
            path.push("*".to_string());
            out.push(("*".to_string(), path));
        }
        syn::UseTree::Group(group) => {
            for item in &group.items {
                use_leaves(prefix, item, out);
            }
        }
    }
}

/// What lib.rs itself defines and re-exports: the names a `crate::X` can mean
/// when `X` is not a module file.
#[derive(Default)]
struct LibRoot {
    defined: BTreeSet<String>,
    reexports: BTreeMap<String, Vec<String>>,
}

impl LibRoot {
    fn parse(lib: &Path) -> Self {
        let mut root = LibRoot::default();
        for item in parse(lib).items {
            if is_test_gated(item_attrs(&item)) {
                continue;
            }
            if let syn::Item::Use(item) = &item {
                root.add_use(item);
            } else if let Some(ident) = item_ident(&item) {
                root.defined.insert(ident.to_string());
            }
        }
        root
    }

    fn add_use(&mut self, item: &syn::ItemUse) {
        let mut leaves = Vec::new();
        use_leaves(&mut Vec::new(), &item.tree, &mut leaves);
        for (name, mut path) in leaves {
            if matches!(path.first().map(String::as_str), Some("crate" | "self")) {
                path.remove(0);
            }
            self.reexports.insert(name, path);
        }
    }
}

/// Where a crate-rooted path lands.
#[derive(Debug, PartialEq)]
enum Target {
    /// A top-level module, with its own file(s).
    Module(String),
    /// Something lib.rs defines or re-exports from outside the crate.
    Root,
}

/// Which crate a file is compiled into. The bin reaches the library only by
/// its name; `crate`, `super` and `self` there mean the bin itself.
#[derive(Clone, Copy, PartialEq)]
enum CrateOf {
    Bin,
    Lib,
}

/// What one file refers to in the library, as paths from the crate root.
#[derive(Default, Clone)]
struct FileScan {
    paths: Vec<Vec<String>>,
    impl_targets: Vec<Vec<String>>,
    errors: Vec<String>,
}

struct References<'a> {
    crate_of: CrateOf,
    here: &'a [String],
    scan: FileScan,
    imports: HashMap<String, Vec<String>>,
    raw_impls: Vec<Vec<String>>,
}

impl References<'_> {
    /// The path from the library root that `segments` names, when it starts
    /// at the library; None for anything else (locals, other crates).
    fn crate_rooted(&self, segments: &[String]) -> Option<Result<Vec<String>, String>> {
        if segments.len() < 2 {
            return None;
        }
        let rest = || segments[1..].to_vec();
        match (self.crate_of, segments[0].as_str()) {
            (CrateOf::Bin, "build_bridge") | (CrateOf::Lib, "crate") => Some(Ok(rest())),
            (CrateOf::Lib, "self") => Some(Ok([self.here, &segments[1..]].concat())),
            (CrateOf::Lib, "super") => Some(self.above(segments)),
            _ => None,
        }
    }

    fn above(&self, segments: &[String]) -> Result<Vec<String>, String> {
        let ups = segments.iter().take_while(|s| *s == "super").count();
        let keep = self
            .here
            .len()
            .checked_sub(ups)
            .ok_or_else(|| format!("`{}` climbs past the crate root", segments.join("::")))?;
        Ok([&self.here[..keep], &segments[ups..]].concat())
    }

    fn record(&mut self, segments: &[String]) {
        match self.crate_rooted(segments) {
            Some(Ok(path)) => self.scan.paths.push(path),
            Some(Err(error)) => self.scan.errors.push(error),
            None => {}
        }
    }

    /// Every `root::a::b` run in a macro's tokens, nested groups included.
    fn record_tokens(&mut self, mut cursor: Cursor) {
        while !cursor.eof() {
            if let Some((inside, _, _, next)) = cursor.any_group() {
                self.record_tokens(inside);
                cursor = next;
            } else if let Some((ident, next)) = cursor.ident() {
                let (segments, next) = path_after(ident.to_string(), next);
                self.record(&segments);
                cursor = next;
            } else if let Some((_, next)) = cursor.token_tree() {
                cursor = next;
            } else {
                break;
            }
        }
    }

    /// The impls' self types, as paths from the crate root where they resolve.
    fn impl_targets(&self) -> Vec<Vec<String>> {
        self.raw_impls
            .iter()
            .filter_map(|raw| match self.crate_rooted(raw) {
                Some(rooted) => rooted.ok(),
                None => self.through_imports(raw),
            })
            .collect()
    }

    fn through_imports(&self, raw: &[String]) -> Option<Vec<String>> {
        match self.imports.get(&raw[0]) {
            Some(imported) => Some([imported.as_slice(), &raw[1..]].concat()),
            // A bare name nobody imported is this module's own type.
            None if self.crate_of == CrateOf::Lib => Some([self.here, raw].concat()),
            None => None,
        }
    }
}

/// `first` followed by as many `::ident` as the tokens carry.
fn path_after(first: String, mut cursor: Cursor) -> (Vec<String>, Cursor) {
    let mut segments = vec![first];
    while let Some((ident, next)) = double_colon(cursor).and_then(|after| after.ident()) {
        segments.push(ident.to_string());
        cursor = next;
    }
    (segments, cursor)
}

fn double_colon(cursor: Cursor) -> Option<Cursor> {
    let (first, after_first) = cursor.punct()?;
    let (second, after_second) = after_first.punct()?;
    (first.as_char() == ':' && second.as_char() == ':').then_some(after_second)
}

impl<'ast> Visit<'ast> for References<'_> {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        if !is_test_gated(item_attrs(item)) {
            syn::visit::visit_item(self, item);
        }
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        let mut leaves = Vec::new();
        use_leaves(&mut Vec::new(), &item.tree, &mut leaves);
        for (name, path) in leaves {
            self.record(&path);
            if let Some(Ok(rooted)) = self.crate_rooted(&path) {
                self.imports.insert(name, rooted);
            }
        }
    }

    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        if let syn::Type::Path(ty) = &*item.self_ty {
            if ty.qself.is_none() {
                let segments = ty.path.segments.iter().map(|s| s.ident.to_string());
                self.raw_impls.push(segments.collect());
            }
        }
        syn::visit::visit_item_impl(self, item);
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        self.record(&segments);
        syn::visit::visit_path(self, path);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let name = mac.path.segments.last().map(|s| s.ident.to_string());
        if let Some(name @ ("include" | "include_str" | "include_bytes")) = name.as_deref() {
            self.scan.errors.push(format!(
                "`{name}!` pulls a file into the relay that no module walk finds"
            ));
        }
        let buffer = TokenBuffer::new2(mac.tokens.clone());
        self.record_tokens(buffer.begin());
        syn::visit::visit_macro(self, mac);
    }
}

/// One crate's `src/`, and how its relay bin reaches into it.
struct SourceTree {
    src: PathBuf,
    lib: LibRoot,
}

impl SourceTree {
    fn new(src: &Path) -> Self {
        SourceTree {
            src: src.to_path_buf(),
            lib: LibRoot::parse(&src.join("lib.rs")),
        }
    }

    /// The module path a source file defines: `transport.rs` is
    /// `[transport]`, `a/b.rs` and `a/b/mod.rs` are `[a, b]`, and the crate
    /// root and the bin are `[]`.
    fn module_path_of(&self, file: &Path) -> Vec<String> {
        let relative = file.strip_prefix(&self.src).expect("a file under src/");
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

    /// `src/<name>.rs` and everything under `src/<name>/`; None when neither
    /// exists.
    fn module_files(&self, name: &str) -> Option<Vec<PathBuf>> {
        let mut files = Vec::new();
        let flat = self.src.join(format!("{name}.rs"));
        if flat.is_file() {
            files.push(flat);
        }
        let dir = self.src.join(name);
        if dir.is_dir() {
            collect_rust_files(&dir, &mut files);
        }
        (!files.is_empty()).then_some(files)
    }

    fn resolve(&self, path: &[String]) -> Result<Target, String> {
        self.resolve_within(path, 8).map_err(|_| {
            format!(
                "`crate::{}` resolves to no file of the crate",
                path.join("::")
            )
        })
    }

    fn resolve_within(&self, path: &[String], hops: usize) -> Result<Target, ()> {
        let top = path.first().ok_or(())?;
        if self.module_files(top).is_some() {
            return Ok(Target::Module(top.clone()));
        }
        if self.lib.defined.contains(top) {
            return Ok(Target::Root);
        }
        let target = self.lib.reexports.get(top).ok_or(())?;
        if !self.is_crate_name(&target[0]) {
            return Ok(Target::Root); // an external crate, re-exported
        }
        let through = [target.as_slice(), &path[1..]].concat();
        self.resolve_within(&through, hops.checked_sub(1).ok_or(())?)
    }

    fn is_crate_name(&self, name: &str) -> bool {
        self.module_files(name).is_some()
            || self.lib.defined.contains(name)
            || self.lib.reexports.contains_key(name)
    }

    fn scan(&self, file: &Path) -> FileScan {
        let here = self.module_path_of(file);
        let crate_of = if file.starts_with(self.src.join("bin")) {
            CrateOf::Bin
        } else {
            CrateOf::Lib
        };
        let mut references = References {
            crate_of,
            here: &here,
            scan: FileScan::default(),
            imports: HashMap::new(),
            raw_impls: Vec::new(),
        };
        references.visit_file(&parse(file));
        let mut scan = references.scan.clone();
        scan.impl_targets = references.impl_targets();
        scan
    }

    fn relay_closure(&self) -> Result<BTreeSet<String>, Vec<String>> {
        let mut walk = Walk::new(self);
        walk.run();
        if !walk.errors.is_empty() {
            return Err(walk.errors);
        }
        Ok(walk.files.iter().map(|file| walk.relative(file)).collect())
    }
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

/// The closure under construction: files reached, the modules they belong to,
/// and every reference that resolved to no file.
struct Walk<'a> {
    tree: &'a SourceTree,
    files: BTreeSet<PathBuf>,
    modules: BTreeSet<String>,
    errors: Vec<String>,
    pending: Vec<PathBuf>,
    scans: HashMap<PathBuf, FileScan>,
}

impl<'a> Walk<'a> {
    fn new(tree: &'a SourceTree) -> Self {
        let mut pending = vec![tree.src.join(RELAY_BIN)];
        // Modules the bin declares for itself (`mod x;` in relay.rs).
        let bin_modules = tree.src.join("bin/relay");
        if bin_modules.is_dir() {
            collect_rust_files(&bin_modules, &mut pending);
        }
        Walk {
            tree,
            files: BTreeSet::new(),
            modules: BTreeSet::new(),
            errors: Vec::new(),
            pending,
            scans: HashMap::new(),
        }
    }

    fn relative(&self, file: &Path) -> String {
        let relative = file.strip_prefix(&self.tree.src).expect("under src/");
        relative.to_string_lossy().into_owned()
    }

    fn scan(&mut self, file: &Path) -> FileScan {
        let tree = self.tree;
        self.scans
            .entry(file.to_path_buf())
            .or_insert_with(|| tree.scan(file))
            .clone()
    }

    /// Follow references to a fixed point, then pull in every module that
    /// implements a type already reached, and follow again.
    fn run(&mut self) {
        loop {
            while let Some(file) = self.pending.pop() {
                if self.files.insert(file.clone()) {
                    self.follow(&file);
                }
            }
            let implementers = self.implementers();
            if implementers.is_empty() {
                return;
            }
            self.pending.extend(implementers);
        }
    }

    fn follow(&mut self, file: &Path) {
        let scan = self.scan(file);
        let at = self.relative(file);
        self.errors
            .extend(scan.errors.iter().map(|e| format!("{at}: {e}")));
        for path in &scan.paths {
            self.reach(&at, path);
        }
    }

    fn reach(&mut self, at: &str, path: &[String]) {
        self.pending.push(self.tree.src.join("lib.rs"));
        match self.tree.resolve(path) {
            Ok(Target::Module(module)) => self.enter(module),
            Ok(Target::Root) => {}
            Err(error) => self.errors.push(format!("{at}: {error}")),
        }
    }

    fn enter(&mut self, module: String) {
        if let Some(files) = self.tree.module_files(&module) {
            if self.modules.insert(module) {
                self.pending.extend(files);
            }
        }
    }

    /// Library files outside the closure with an `impl` on a type inside it:
    /// methods the relay can call that live in another module.
    fn implementers(&mut self) -> Vec<PathBuf> {
        let mut library = Vec::new();
        collect_rust_files(&self.tree.src, &mut library);
        library.retain(|file| {
            !file.starts_with(self.tree.src.join("bin")) && !self.files.contains(file)
        });
        library
            .into_iter()
            .filter(|file| self.implements_a_reached_type(file))
            .collect()
    }

    fn implements_a_reached_type(&mut self, file: &Path) -> bool {
        self.scan(file).impl_targets.iter().any(|target| {
            matches!(self.tree.resolve(target), Ok(Target::Module(m)) if self.modules.contains(&m))
        })
    }
}

fn bridge_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The list's entries, split into the crate's sources (relative to
/// `bridge/src/`) and everything else.
fn listed() -> (BTreeSet<String>, BTreeSet<String>) {
    let list = bridge_dir().join("../.github/relay-sources");
    let text = fs::read_to_string(&list).expect("read .github/relay-sources");
    let (sources, other): (BTreeSet<String>, BTreeSet<String>) = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .partition(|path| path.starts_with("bridge/src/"));
    let sources = sources
        .iter()
        .map(|path| path["bridge/src/".len()..].to_string())
        .collect();
    (sources, other)
}

#[test]
fn relay_sources_lists_exactly_the_modules_the_relay_is_built_from() {
    let (listed_rust, _) = listed();
    let derived = SourceTree::new(&bridge_dir().join("src"))
        .relay_closure()
        .unwrap_or_else(|errors| panic!("the relay's references do not resolve: {errors:#?}"));
    let unlisted: Vec<_> = derived.difference(&listed_rust).collect();
    let stale: Vec<_> = listed_rust.difference(&derived).collect();
    assert!(
        unlisted.is_empty() && stale.is_empty(),
        "the relay is built from bridge/src/{derived:?}. Add to .github/relay-sources (or CI \
         will not redeploy the relay when they change): {unlisted:?}. Remove from it (or \
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

// The walk on small fixture crates: each case is a way the relay can come to
// depend on a file, and the walk must find it or refuse to answer.

const FIXTURE_LIB: &str = "pub mod priority;\npub mod relay_server;\npub mod transport;\n";
const FIXTURE_PRIORITY: &str = "pub const AGENTS_SLICE: &str = \"agents\";\n";
const BASELINE: [&str; 4] = ["bin/relay.rs", "lib.rs", "relay_server.rs", "transport.rs"];

/// A crate whose relay reaches relay_server, which reaches transport, with
/// `run_body` inside relay_server's `run`, and any file replaced by `extra`.
fn fixture_closure(
    run_body: &str,
    extra: &[(&str, &str)],
) -> Result<BTreeSet<String>, Vec<String>> {
    let dir = tempfile::tempdir().expect("tempdir");
    let relay_server = format!(
        "use crate::transport;\npub fn run() {{\n    transport::seal();\n    {run_body}\n}}\n"
    );
    let mut files: BTreeMap<&str, String> = BTreeMap::from([
        (
            "bin/relay.rs",
            "use build_bridge::relay_server;\nfn main() { relay_server::run(); }\n".to_string(),
        ),
        ("lib.rs", FIXTURE_LIB.to_string()),
        ("relay_server.rs", relay_server),
        (
            "transport.rs",
            "pub struct KeyPair;\npub fn seal() {}\n".to_string(),
        ),
        ("priority.rs", FIXTURE_PRIORITY.to_string()),
    ]);
    for (path, text) in extra {
        files.insert(path, text.to_string());
    }
    for (path, text) in &files {
        let file = dir.path().join("src").join(path);
        fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
        fs::write(file, text).expect("write fixture");
    }
    SourceTree::new(&dir.path().join("src")).relay_closure()
}

fn with_priority() -> BTreeSet<String> {
    BASELINE
        .iter()
        .chain(&["priority.rs"])
        .map(|s| s.to_string())
        .collect()
}

fn baseline() -> BTreeSet<String> {
    BASELINE.iter().map(|s| s.to_string()).collect()
}

#[test]
fn the_walk_follows_the_relay_through_the_library() {
    assert_eq!(fixture_closure("", &[]), Ok(baseline()));
}

#[test]
fn a_crate_path_in_code_reaches_its_module() {
    let body = "let _ = crate::priority::AGENTS_SLICE;";
    assert_eq!(fixture_closure(body, &[]), Ok(with_priority()));
}

#[test]
fn a_crate_path_inside_a_macro_reaches_its_module() {
    let body = "let _ = format!(\"{}\", crate::priority::AGENTS_SLICE);";
    assert_eq!(fixture_closure(body, &[]), Ok(with_priority()));
}

#[test]
fn a_super_path_nested_in_macros_reaches_its_module() {
    let body = "let _ = vec![format!(\"{}\", super::priority::AGENTS_SLICE)];";
    assert_eq!(fixture_closure(body, &[]), Ok(with_priority()));
}

#[test]
fn a_lib_reexport_reaches_the_module_that_defines_it() {
    let lib = format!("{FIXTURE_LIB}pub use priority::AGENTS_SLICE;\n");
    let body = "let _ = crate::AGENTS_SLICE;";
    assert_eq!(
        fixture_closure(body, &[("lib.rs", &lib)]),
        Ok(with_priority())
    );
}

#[test]
fn an_item_lib_rs_defines_needs_no_other_file() {
    let lib = format!("{FIXTURE_LIB}pub const LIMIT: usize = 1;\n");
    assert_eq!(
        fixture_closure("let _ = crate::LIMIT;", &[("lib.rs", &lib)]),
        Ok(baseline())
    );
}

#[test]
fn an_impl_on_a_relay_type_pulls_in_its_module() {
    let by_path = format!(
        "{FIXTURE_PRIORITY}impl crate::transport::KeyPair {{ pub fn probe(&self) {{}} }}\n"
    );
    assert_eq!(
        fixture_closure("", &[("priority.rs", &by_path)]),
        Ok(with_priority())
    );
    let by_import = format!("use crate::transport::KeyPair;\n{FIXTURE_PRIORITY}impl KeyPair {{ pub fn probe(&self) {{}} }}\n");
    assert_eq!(
        fixture_closure("", &[("priority.rs", &by_import)]),
        Ok(with_priority())
    );
}

#[test]
fn test_only_references_are_not_the_relay_s() {
    let tests = "#[cfg(test)]\nmod tests {\n    use crate::priority::AGENTS_SLICE;\n}\n";
    let relay_server =
        format!("use crate::transport;\npub fn run() {{ transport::seal(); }}\n{tests}");
    assert_eq!(
        fixture_closure("", &[("relay_server.rs", &relay_server)]),
        Ok(baseline())
    );
}

fn assert_refused(result: Result<BTreeSet<String>, Vec<String>>, naming: &str) {
    match result {
        Err(errors) => assert!(
            errors.iter().any(|e| e.contains(naming)),
            "refused, but not for {naming}: {errors:?}"
        ),
        Ok(files) => panic!("resolved {files:?} instead of refusing {naming}"),
    }
}

#[test]
fn a_crate_path_that_resolves_to_no_file_is_refused() {
    let result = fixture_closure("let _ = format!(\"{}\", crate::nowhere::X);", &[]);
    assert_refused(result, "crate::nowhere::X");
}

#[test]
fn a_glob_of_the_crate_root_is_refused() {
    let relay_server = "use crate::*;\npub fn run() {}\n";
    assert_refused(
        fixture_closure("", &[("relay_server.rs", relay_server)]),
        "crate::*",
    );
}

#[test]
fn a_file_included_by_macro_is_refused() {
    let result = fixture_closure("let _ = include_str!(\"banner.txt\");", &[]);
    assert_refused(result, "include_str!");
}
