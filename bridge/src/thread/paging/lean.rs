//! Lean conversation pages: one newest item per run and a whole-run digest.
use super::*;

impl Thread {
    /// Lean paging needs the complete edges of every touched run. A truncated
    /// tail cannot certify those edges, even if its newest messages are held.
    pub fn lean_page_reaches_stored_history(&self) -> bool {
        self.earlier_item_count > 0
    }

    /// The lean projection measures each direction in messages. Activity and
    /// lifecycle markers ride with those messages; a cursor clips shipped
    /// items but never clips the digest's census of a touched run.
    pub fn wire_value_page_newest_activity(
        &self,
        after_sequence: Option<u64>,
        before_sequence: Option<u64>,
        limit: usize,
        newest: bool,
    ) -> Value {
        let window: Vec<&ThreadItem> = self
            .items
            .iter()
            .filter(|item| {
                item.sequence() > after_sequence.unwrap_or(0)
                    && item.sequence() < before_sequence.unwrap_or(u64::MAX)
            })
            .collect();
        let span = lean_window(window, limit.max(1), after_sequence.is_none() || newest);
        let cut = lean_cut(&self.items, &span);
        let has_more = match (after_sequence, newest) {
            (Some(_), false) => cut.items.last().is_some_and(|last| {
                self.items
                    .iter()
                    .any(|item| item.sequence() > last.sequence())
            }),
            (after, _) => cut.items.first().is_some_and(|first| {
                self.items.iter().any(|item| {
                    item.sequence() < first.sequence() && item.sequence() > after.unwrap_or(0)
                })
            }),
        };
        self.wire_value_of_page(&cut, has_more)
    }
}

/// Oldest forward spans stop just before the next message after their limit,
/// so even a long live run remains one indivisible folded row.
fn lean_window(window: Vec<&ThreadItem>, limit: usize, newest: bool) -> Vec<&ThreadItem> {
    if newest {
        let start = window.len() - page_span(window.iter().rev().copied(), limit);
        return window.into_iter().skip(start).collect();
    }
    let end = window
        .iter()
        .enumerate()
        .filter(|(_, item)| item.counts_toward_page())
        .nth(limit)
        .map_or(window.len(), |(index, _)| index);
    window.into_iter().take(end).collect()
}

/// Walk the complete resident conversation to preserve the true edges and
/// last call of any run the selected span touches. Only selected rows ship.
fn lean_cut<'a>(items: &'a [ThreadItem], span: &[&'a ThreadItem]) -> PageCut<&'a ThreadItem> {
    let mut cut = PageCut {
        items: Vec::new(),
        digests: Vec::new(),
    };
    let (Some(first), Some(last)) = (span.first(), span.last()) else {
        return cut;
    };
    let from = first.sequence();
    let through = last.sequence();
    let mut run = Vec::new();
    for item in items {
        if item.is_activity() {
            run.push(item);
        } else {
            lean_fold(&mut cut, &run, from, through);
            run.clear();
            if (from..=through).contains(&item.sequence()) {
                cut.items.push(item);
            }
        }
    }
    lean_fold(&mut cut, &run, from, through);
    cut
}

fn lean_fold<'a>(
    cut: &mut PageCut<&'a ThreadItem>,
    run: &[&'a ThreadItem],
    from: u64,
    through: u64,
) {
    let selected: Vec<&ThreadItem> = run
        .iter()
        .copied()
        .filter(|item| (from..=through).contains(&item.sequence()))
        .collect();
    let ships = run_items_shipped_with_activity(selected.len(), usize::MAX, PageActivity::Newest);
    if ships == 0 {
        return;
    }
    cut.items.extend(selected.into_iter().rev().take(ships));
    cut.digests.push(ActivityDigest {
        from_sequence: run.first().unwrap().sequence(),
        through_sequence: run.last().unwrap().sequence(),
        rows: run.len() as u64,
        tool_calls: run.iter().filter(|item| item.is_tool_call()).count() as u64,
        last_tool_call: run
            .iter()
            .rev()
            .find_map(|item| item.tool_call().map(LastToolCall::of)),
    });
}
