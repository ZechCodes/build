//! Inventory guard for SQL writer ownership, including formatted table writes.
//! Semantic producer tests accompany this guard; classification alone is not
//! evidence that a hook runs or that publication shares the source commit.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
use syn::visit::{self, Visit};

const COVERED: &[&str] = &[
    "tracker_tasks",
    "tracker_comments",
    "tracker_events",
    "tracker_task_dependencies",
    "reviews",
    "tracker_delivery_intents",
    "agents", "thread_items", "attention", "captures", "implementations", "tasks",
    "archived_worktrees", "inbox_retained_messages", "inbox_retained_run_messages",
];

#[derive(Default)]
struct Body {
    tables: BTreeSet<String>,
    named: BTreeSet<String>,
    dynamic: bool,
    calls: BTreeSet<String>,
}
fn mutation_tables(raw: &str) -> (BTreeSet<String>, bool) {
    let words: Vec<_> = raw
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '{'))
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let mut tables = BTreeSet::new();
    let mut dynamic = false;
    for (i, word) in words.iter().enumerate() {
        let next = match word.as_str() {
            "update" => words.get(i + 1),
            "into"
                if i > 0
                    && words[..i]
                        .iter()
                        .rev()
                        .take(3)
                        .any(|w| w == "insert" || w == "replace") =>
            {
                words.get(i + 1)
            }
            "from" if i > 0 && words[i - 1] == "delete" => words.get(i + 1),
            _ => None,
        };
        if let Some(table) = next {
            dynamic |= table.starts_with('{');
            if COVERED.contains(&table.as_str()) {
                tables.insert(table.clone());
            }
        }
    }
    (tables, dynamic)
}
struct BodyVisitor<'a> {
    body: Body,
    constants: &'a BTreeMap<String, String>,
}
impl<'ast> Visit<'ast> for BodyVisitor<'_> {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = call.func.as_ref() {
            if let Some(name) = path.path.segments.last() {
                self.body.calls.insert(name.ident.to_string());
            }
        }
        visit::visit_expr_call(self, call);
    }
    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        self.body.calls.insert(call.method.to_string());
        visit::visit_expr_method_call(self, call);
    }
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        use syn::parse::Parser;
        if let Ok(args) = syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated
            .parse2(mac.tokens.clone())
        {
            // syn's default visitor leaves macro token trees opaque. SQL in
            // format!("DELETE FROM {table}") must enter the same inventory.
            for arg in &args {
                self.visit_expr(arg);
            }
        }
    }
    fn visit_lit_str(&mut self, lit: &'ast syn::LitStr) {
        let value = lit.value();
        let (tables, dynamic) = mutation_tables(&value);
        self.body.tables.extend(tables);
        self.body.dynamic |= dynamic;
        if COVERED.contains(&value.as_str()) {
            self.body.named.insert(value);
        }
    }
    fn visit_expr_path(&mut self, path: &'ast syn::ExprPath) {
        if let Some(name) = path.path.get_ident() {
            if let Some(value) = self.constants.get(&name.to_string()) {
                let (tables, dynamic) = mutation_tables(value);
                self.body.tables.extend(tables);
                self.body.dynamic |= dynamic;
            }
        }
    }
}
struct Inventory {
    file: String,
    constants: BTreeMap<String, String>,
    writers: BTreeSet<String>,
    hooks: BTreeMap<String, BTreeSet<String>>,
}
impl Inventory {
    fn function(&mut self, name: &syn::Ident, block: &syn::Block) {
        let mut visitor = BodyVisitor {
            body: Body::default(),
            constants: &self.constants,
        };
        visitor.visit_block(block);
        let mut body = visitor.body;
        self.hooks
            .insert(format!("{}::{name}", self.file), body.calls);
        if body.dynamic {
            body.tables.extend(body.named);
            body.tables.insert("<dynamic>".into());
        }
        for table in body.tables {
            self.writers
                .insert(format!("{}::{name} -> {table}", self.file));
        }
    }
}
fn test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().segments.last().is_some_and(|s| s.ident == "test")
            || (a.path().is_ident("cfg") && quote_cfg(a).contains("test"))
    })
}
fn quote_cfg(attr: &syn::Attribute) -> String {
    match &attr.meta {
        syn::Meta::List(list) => list.tokens.to_string(),
        _ => String::new(),
    }
}
impl<'ast> Visit<'ast> for Inventory {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if !test_only(&item.attrs) {
            visit::visit_item_mod(self, item);
        }
    }
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if !test_only(&item.attrs) {
            self.function(&item.sig.ident, &item.block);
        }
    }
    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if !test_only(&item.attrs) {
            self.function(&item.sig.ident, &item.block);
        }
    }
}
fn files(root: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path.file_name().unwrap() != "tests" {
                files(&path, out);
            }
        } else if path.extension().is_some_and(|ext| ext == "rs") && path.file_name().is_some_and(|name| name != "tests.rs" && !name.to_string_lossy().ends_with("_tests.rs")) {
            out.push(path);
        }
    }
}
#[test]
fn every_canonical_sql_writer_has_an_audited_publication_owner() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/store");
    let mut paths = Vec::new();
    files(&root, &mut paths);
    let mut actual = BTreeSet::new();
    let mut hooks = BTreeMap::new();
    for path in paths {
        if path.components().any(|c| c.as_os_str() == "sync") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        let syntax = syn::parse_file(&source).unwrap();
        let constants = syntax
            .items
            .iter()
            .filter_map(|item| {
                if let syn::Item::Const(item) = item {
                    if let syn::Expr::Lit(lit) = item.expr.as_ref() {
                        if let syn::Lit::Str(value) = &lit.lit {
                            return Some((item.ident.to_string(), value.value()));
                        }
                    }
                }
                None
            })
            .collect();
        let mut visitor = Inventory {
            file: path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            constants,
            writers: BTreeSet::new(),
            hooks: BTreeMap::new(),
        };
        visitor.visit_file(&syntax);
        actual.extend(visitor.writers);
        hooks.extend(visitor.hooks);
    }
    // Startup migrations are classified separately from transactional producers.
    let expected: BTreeSet<String> = [
        "conversation_reset.rs::reset_conversation -> agents",
        "conversation_reset.rs::reset_conversation -> attention",
        "conversation_reset.rs::reset_conversation -> implementations",
        "conversation_reset.rs::reset_conversation -> tasks",
        "conversation_reset.rs::reset_conversation -> thread_items",
        "delivery_intents.rs::write -> tracker_delivery_intents",
        "delivery_intents/lifecycle.rs::cancel_project -> tracker_delivery_intents",
        "entities.rs::classify_stored_items -> thread_items",
        "entities.rs::clear_retained_project_messages -> inbox_retained_messages",
        "entities.rs::delete_capture -> captures",
        "entities.rs::delete_plan -> agents",
        "entities.rs::delete_plan -> implementations",
        "entities.rs::delete_plan -> inbox_retained_run_messages",
        "entities.rs::delete_plan -> tasks",
        "entities.rs::delete_plan -> thread_items",
        "entities.rs::delete_run -> agents",
        "entities.rs::delete_run -> implementations",
        "entities.rs::delete_run -> inbox_retained_run_messages",
        "entities.rs::delete_run -> thread_items",
        "entities.rs::delete_run_retaining_inbox_messages -> agents",
        "entities.rs::delete_run_retaining_inbox_messages -> implementations",
        "entities.rs::delete_run_retaining_inbox_messages -> inbox_retained_messages",
        "entities.rs::delete_run_retaining_inbox_messages -> inbox_retained_run_messages",
        "entities.rs::delete_run_retaining_inbox_messages -> thread_items",
        "entities.rs::migrate_agents_to_v6 -> agents",
        "entities.rs::retain_agent_message_times -> inbox_retained_run_messages",
        "entities.rs::save_archived_worktree -> archived_worktrees",
        "entities.rs::save_attention -> attention",
        "entities.rs::save_capture -> captures",
        "entities.rs::write_agents -> agents",
        "entities.rs::write_agents -> thread_items",
        "entities.rs::write_run -> implementations",
        "entities.rs::write_task -> tasks",
        "operations.rs::acknowledge_messages -> agents",
        "operations.rs::acknowledge_messages -> thread_items",
        "operations/delivery_status.rs::write_status -> agents",
        "operations/delivery_status.rs::write_status -> thread_items",
        "retention.rs::prune_conversation_batch -> thread_items",
        "reviews.rs::write_header -> reviews",
        "task_rename.rs::rewrite_table -> <dynamic>",
        "task_summaries.rs::reset_table -> <dynamic>",
        "terminal_attention.rs::save_terminal_interaction -> attention",
        "tracker.rs::append_activity -> tracker_comments",
        "tracker.rs::append_activity -> tracker_events",
        "tracker.rs::delete_tracker_tasks_of_project -> <dynamic>",
        "tracker.rs::delete_tracker_tasks_of_project -> reviews",
        "tracker.rs::delete_tracker_tasks_of_project -> tracker_comments",
        "tracker.rs::delete_tracker_tasks_of_project -> tracker_events",
        "tracker.rs::delete_tracker_tasks_of_project -> tracker_tasks",
        "tracker.rs::write_tracker_task -> tracker_tasks",
        "waiting_on/graph.rs::add_edges -> tracker_task_dependencies",
        "waiting_on/graph.rs::edit -> tracker_task_dependencies",
        "waiting_on/graph.rs::edit -> tracker_tasks",
        "waiting_on/reconciliation.rs::reconcile_task -> tracker_tasks",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_eq!(actual,expected,"new/removed SQL writer: classify its source-transaction hook or startup/private-only boundary, then add a semantic producer case");
    for (owner, required) in [
        (
            "tracker.rs::write_tracker_task",
            &["touch_task", "task_written"][..],
        ),
        ("tracker.rs::append_activity", &["history_append"][..]),
        (
            "tracker.rs::delete_tracker_tasks_of_project",
            &["task_remove", "dependency_refresh"][..],
        ),
        (
            "waiting_on/graph.rs::edit",
            &[
                "add_edges",
                "append_activity",
                "task_refresh",
                "dependency_refresh",
            ][..],
        ),
        (
            "waiting_on/reconciliation.rs::reconcile_task",
            &["append_activity", "task_refresh"][..],
        ),
        ("delivery_intents.rs::write", &["metadata_refresh"][..]),
        (
            "delivery_intents/lifecycle.rs::cancel_project",
            &["metadata_refresh"][..],
        ),
        (
            "reviews.rs::write_header",
            &["touch_task", "metadata_refresh"][..],
        ),
        (
            "writer.rs::transaction_with_publications",
            &["collect_tasks", "collect_device", "publish", "commit"][..],
        ),
    ] {
        assert_hooks(&hooks, owner, required);
    }
    for name in [
        "task_written_unchecked",
        "task_remove",
        "task_dependents_refresh",
        "metadata_refresh",
        "dependency_refresh",
        "history_append",
        "history_repair",
    ] {
        assert_hooks(
            &hooks,
            &format!("task_summaries/maintenance.rs::{name}"),
            &["touch_task"],
        );
    }
}
fn assert_hooks(hooks: &BTreeMap<String, BTreeSet<String>>, owner: &str, required: &[&str]) {
    let calls = hooks
        .get(owner)
        .unwrap_or_else(|| panic!("missing publication owner {owner}"));
    for hook in required {
        assert!(calls.contains(*hook), "{owner} lost its {hook} boundary");
    }
}
