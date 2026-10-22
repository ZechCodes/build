//! Persistent treap keyed by creation sequence. Its deterministic mixed-key
//! priorities keep appends balanced without mutable shared nodes or RNG state.
use crate::thread::{ThreadItem, UnreadSummary};
use std::sync::Arc;

type Link = Option<Arc<Node>>;

pub(super) struct Node {
    pub item: Arc<ThreadItem>,
    pub left: Link,
    pub right: Link,
    priority: u64,
    latest: u64,
    pub count: u64,
    unread: UnreadSummary,
}

impl Node {
    pub fn last_sequence(root: &Link) -> u64 {
        root.as_ref().map_or(0, |node| node.latest)
    }

    fn new(item: Arc<ThreadItem>, left: Link, right: Link) -> Arc<Self> {
        let latest = item
            .latest_sequence()
            .max(left.as_ref().map_or(0, |node| node.latest))
            .max(right.as_ref().map_or(0, |node| node.latest));
        let count = 1
            + left.as_ref().map_or(0, |node| node.count)
            + right.as_ref().map_or(0, |node| node.count);
        let unread = combine_unread(
            combine_unread(
                left.as_ref()
                    .map_or(UnreadSummary::default(), |node| node.unread),
                item.attention_reason()
                    .map_or(UnreadSummary::default(), |reason| UnreadSummary {
                        count: 1,
                        reason: Some(reason),
                    }),
            ),
            right
                .as_ref()
                .map_or(UnreadSummary::default(), |node| node.unread),
        );
        Arc::new(Self {
            priority: priority(item.sequence()),
            item,
            left,
            right,
            latest,
            count,
            unread,
        })
    }

    pub fn unread_after(root: &Link, cursor: u64) -> UnreadSummary {
        let Some(node) = root else {
            return UnreadSummary::default();
        };
        if node.item.sequence() <= cursor {
            return Self::unread_after(&node.right, cursor);
        }
        let own = node
            .item
            .attention_reason()
            .map_or(UnreadSummary::default(), |reason| UnreadSummary {
                count: 1,
                reason: Some(reason),
            });
        combine_unread(
            combine_unread(Self::unread_after(&node.left, cursor), own),
            node.right
                .as_ref()
                .map_or(UnreadSummary::default(), |node| node.unread),
        )
    }

    pub fn collect_page<'a>(
        root: &'a Link,
        counted: &mut usize,
        limit: usize,
        span: &mut Vec<&'a ThreadItem>,
    ) {
        let Some(node) = root.as_ref().filter(|_| *counted < limit) else {
            return;
        };
        Self::collect_page(&node.right, counted, limit, span);
        if *counted == limit {
            return;
        }
        *counted += usize::from(node.item.counts_toward_page());
        span.push(&node.item);
        Self::collect_page(&node.left, counted, limit, span);
    }

    pub fn collect<'a>(root: &'a Link, items: &mut Vec<&'a ThreadItem>) {
        if let Some(node) = root {
            Self::collect(&node.left, items);
            items.push(&node.item);
            Self::collect(&node.right, items);
        }
    }

    pub fn collect_from<'a>(root: &'a Link, floor: u64, items: &mut Vec<&'a ThreadItem>) {
        let Some(node) = root else {
            return;
        };
        if node.item.sequence() < floor {
            Self::collect_from(&node.right, floor, items);
            return;
        }
        Self::collect_from(&node.left, floor, items);
        items.push(&node.item);
        Self::collect(&node.right, items);
    }

    pub fn collect_after<'a>(root: &'a Link, cursor: u64, items: &mut Vec<&'a ThreadItem>) {
        let Some(node) = root.as_ref().filter(|node| node.latest > cursor) else {
            return;
        };
        Self::collect_after(&node.left, cursor, items);
        if node.item.latest_sequence() > cursor {
            items.push(&node.item);
        }
        Self::collect_after(&node.right, cursor, items);
    }
}

pub(super) fn insert(root: &Link, item: Arc<ThreadItem>) -> Arc<Node> {
    let Some(node) = root else {
        return Node::new(item, None, None);
    };
    match item.sequence().cmp(&node.item.sequence()) {
        std::cmp::Ordering::Equal => Node::new(item, node.left.clone(), node.right.clone()),
        std::cmp::Ordering::Less => {
            let left = insert(&node.left, item);
            if left.priority > node.priority {
                let right = Node::new(node.item.clone(), left.right.clone(), node.right.clone());
                Node::new(left.item.clone(), left.left.clone(), Some(right))
            } else {
                Node::new(node.item.clone(), Some(left), node.right.clone())
            }
        }
        std::cmp::Ordering::Greater => {
            let right = insert(&node.right, item);
            if right.priority > node.priority {
                let left = Node::new(node.item.clone(), node.left.clone(), right.left.clone());
                Node::new(right.item.clone(), Some(left), right.right.clone())
            } else {
                Node::new(node.item.clone(), node.left.clone(), Some(right))
            }
        }
    }
}

fn priority(sequence: u64) -> u64 {
    let mixed = sequence.wrapping_add(0x9e3779b97f4a7c15);
    let mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    let mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d049bb133111eb);
    mixed ^ (mixed >> 31)
}

fn combine_unread(older: UnreadSummary, newer: UnreadSummary) -> UnreadSummary {
    UnreadSummary {
        count: older.count + newer.count,
        reason: newer.reason.or(older.reason),
    }
}

pub(super) fn remove(root: &Link, sequence: u64) -> Link {
    let node = root.as_ref()?;
    match sequence.cmp(&node.item.sequence()) {
        std::cmp::Ordering::Equal => merge(&node.left, &node.right),
        std::cmp::Ordering::Less => Some(Node::new(
            node.item.clone(),
            remove(&node.left, sequence),
            node.right.clone(),
        )),
        std::cmp::Ordering::Greater => Some(Node::new(
            node.item.clone(),
            node.left.clone(),
            remove(&node.right, sequence),
        )),
    }
}

fn merge(left: &Link, right: &Link) -> Link {
    match (left, right) {
        (None, _) => right.clone(),
        (_, None) => left.clone(),
        (Some(left_node), Some(right_node)) if left_node.priority > right_node.priority => {
            Some(Node::new(
                left_node.item.clone(),
                left_node.left.clone(),
                merge(&left_node.right, right),
            ))
        }
        (Some(_), Some(right_node)) => Some(Node::new(
            right_node.item.clone(),
            merge(left, &right_node.left),
            right_node.right.clone(),
        )),
    }
}
