//! Immutable JSON fragments composed without copying their rendered leaves.

use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct SharedJson(Arc<Node>);

#[derive(Debug)]
struct Node {
    kind: Kind,
    digests: BTreeMap<DigestKey, u64>,
}

#[derive(Debug)]
enum Kind {
    Leaf(Arc<Value>),
    Object(BTreeMap<String, SharedJson>),
    Array(Vec<SharedJson>),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DigestKey {
    thread_id: String,
    generation: u64,
}

impl Default for SharedJson {
    fn default() -> Self {
        Self::value(Value::Null)
    }
}

impl SharedJson {
    pub fn value(value: Value) -> Self {
        Self::from_shared(Arc::new(value))
    }

    pub fn from_shared(value: Arc<Value>) -> Self {
        Self::new(Kind::Leaf(value))
    }

    pub fn object(fields: impl IntoIterator<Item = (String, Self)>) -> Self {
        Self::new(Kind::Object(fields.into_iter().collect()))
    }

    pub fn array(values: impl IntoIterator<Item = Self>) -> Self {
        Self::new(Kind::Array(values.into_iter().collect()))
    }

    fn new(kind: Kind) -> Self {
        let mut digests = BTreeMap::new();
        match &kind {
            Kind::Leaf(value) => collect_value_digests(value, &mut digests),
            Kind::Object(fields) => {
                for child in fields.values() {
                    merge_digests(&mut digests, &child.0.digests);
                }
                record_digest(&mut digests, shared_object_digest(fields));
            }
            Kind::Array(values) => {
                for child in values {
                    merge_digests(&mut digests, &child.0.digests);
                }
            }
        }
        Self(Arc::new(Node { kind, digests }))
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
            || matches!((&self.0.kind, &other.0.kind), (Kind::Leaf(a), Kind::Leaf(b)) if Arc::ptr_eq(a, b))
    }

    pub fn leaf(&self) -> Option<&Value> {
        match &self.0.kind {
            Kind::Leaf(value) => Some(value),
            _ => None,
        }
    }

    pub fn field(&self, field: &str) -> Option<Self> {
        match &self.0.kind {
            Kind::Object(fields) => fields.get(field).cloned(),
            _ => None,
        }
    }

    pub fn fields(&self) -> Option<&BTreeMap<String, Self>> {
        match &self.0.kind {
            Kind::Object(fields) => Some(fields),
            _ => None,
        }
    }

    pub fn elements(&self) -> Option<&[Self]> {
        match &self.0.kind {
            Kind::Array(values) => Some(values),
            _ => None,
        }
    }

    /// Consumer boundary only. Published nodes never retain this full copy.
    pub fn materialize(&self) -> Value {
        #[cfg(test)]
        crate::app::snapshot_projection::work_counts::detail_scans::materialized_node();
        match &self.0.kind {
            Kind::Leaf(value) => value.as_ref().clone(),
            Kind::Object(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(key, child)| (key.clone(), child.materialize()))
                    .collect(),
            ),
            Kind::Array(values) => Value::Array(values.iter().map(Self::materialize).collect()),
        }
    }

    pub fn equals_value(&self, value: &Value) -> bool {
        match &self.0.kind {
            Kind::Leaf(leaf) => leaf.as_ref() == value,
            Kind::Object(fields) => value.as_object().is_some_and(|other| {
                fields.len() == other.len()
                    && fields.iter().all(|(key, child)| {
                        other
                            .get(key)
                            .is_some_and(|value| child.equals_value(value))
                    })
            }),
            Kind::Array(values) => value.as_array().is_some_and(|other| {
                values.len() == other.len()
                    && values.iter().zip(other).all(|(a, b)| a.equals_value(b))
            }),
        }
    }
}

impl PartialEq for SharedJson {
    fn eq(&self, other: &Self) -> bool {
        if self.ptr_eq(other) {
            return true;
        }
        match (&self.0.kind, &other.0.kind) {
            (_, Kind::Leaf(value)) => self.equals_value(value),
            (Kind::Leaf(value), _) => other.equals_value(value),
            (Kind::Object(a), Kind::Object(b)) => a == b,
            (Kind::Array(a), Kind::Array(b)) => a == b,
            _ => false,
        }
    }
}

fn merge_digests(target: &mut BTreeMap<DigestKey, u64>, source: &BTreeMap<DigestKey, u64>) {
    for (key, sequence) in source {
        record_digest(target, Some((key.clone(), *sequence)));
    }
}

fn record_digest(target: &mut BTreeMap<DigestKey, u64>, digest: Option<(DigestKey, u64)>) {
    if let Some((key, sequence)) = digest {
        target
            .entry(key)
            .and_modify(|previous| *previous = (*previous).min(sequence))
            .or_insert(sequence);
    }
}

fn value_object_digest(object: &Map<String, Value>) -> Option<(DigestKey, u64)> {
    Some((
        DigestKey {
            thread_id: object.get("thread_id")?.as_str()?.to_owned(),
            generation: object.get("thread_generation_revision")?.as_u64()?,
        },
        object.get("last_sequence")?.as_u64()?,
    ))
}

fn shared_object_digest(object: &BTreeMap<String, SharedJson>) -> Option<(DigestKey, u64)> {
    Some((
        DigestKey {
            thread_id: object.get("thread_id")?.leaf()?.as_str()?.to_owned(),
            generation: object.get("thread_generation_revision")?.leaf()?.as_u64()?,
        },
        object.get("last_sequence")?.leaf()?.as_u64()?,
    ))
}

fn collect_value_digests(value: &Value, digests: &mut BTreeMap<DigestKey, u64>) {
    #[cfg(test)]
    crate::app::snapshot_projection::work_counts::named_branch::indexed_value();
    #[cfg(test)]
    crate::app::snapshot_projection::work_counts::detail_scans::indexed_value();
    match value {
        Value::Object(object) => {
            record_digest(digests, value_object_digest(object));
            for child in object.values() {
                collect_value_digests(child, digests);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_value_digests(child, digests);
            }
        }
        _ => {}
    }
}

/// One receipt patches all wire aliases and retained semantic rows through the
/// same memo. The minimum sequence index rejects unrelated leaves in O(log n).
pub struct DigestPatch<'a> {
    thread_id: &'a str,
    generation: u64,
    sequence: u64,
    key: DigestKey,
    memo: HashMap<(bool, usize), (SharedJson, SharedJson)>,
    #[cfg(test)]
    copied_leaves: usize,
}

impl<'a> DigestPatch<'a> {
    pub fn new(thread_id: &'a str, generation: u64, sequence: u64) -> Self {
        Self {
            thread_id,
            generation,
            sequence,
            key: DigestKey {
                thread_id: thread_id.to_owned(),
                generation,
            },
            memo: HashMap::new(),
            #[cfg(test)]
            copied_leaves: 0,
        }
    }

    pub fn apply(&mut self, value: &SharedJson) -> Option<SharedJson> {
        if value
            .0
            .digests
            .get(&self.key)
            .is_none_or(|held| *held >= self.sequence)
        {
            return None;
        }
        let identity = match &value.0.kind {
            Kind::Leaf(leaf) => (true, Arc::as_ptr(leaf) as usize),
            _ => (false, Arc::as_ptr(&value.0) as usize),
        };
        if let Some((_, patched)) = self.memo.get(&identity) {
            return Some(patched.clone());
        }
        let next = match &value.0.kind {
            Kind::Leaf(leaf) => {
                let mut next = leaf.as_ref().clone();
                patch_value_digests(&mut next, self.thread_id, self.generation, self.sequence);
                #[cfg(test)]
                {
                    self.copied_leaves += 1;
                }
                SharedJson::value(next)
            }
            Kind::Object(fields) => self.patch_object(fields),
            Kind::Array(values) => SharedJson::array(
                values
                    .iter()
                    .map(|child| self.apply(child).unwrap_or_else(|| child.clone())),
            ),
        };
        // Retain the source allocation while its address is a memo key.
        self.memo.insert(identity, (value.clone(), next.clone()));
        Some(next)
    }

    fn patch_object(&mut self, fields: &BTreeMap<String, SharedJson>) -> SharedJson {
        let direct = shared_object_digest(fields).is_some_and(|(key, held)| {
            key.thread_id == self.thread_id
                && key.generation == self.generation
                && held < self.sequence
        });
        SharedJson::object(fields.iter().map(|(key, child)| {
            let value = if direct && key == "last_sequence" {
                SharedJson::value(Value::from(self.sequence))
            } else {
                self.apply(child).unwrap_or_else(|| child.clone())
            };
            (key.clone(), value)
        }))
    }

    #[cfg(test)]
    pub fn copied_leaves(&self) -> usize {
        self.copied_leaves
    }
}

#[cfg(test)]
pub fn value_needs_digest_patch(
    value: &Value,
    thread: &str,
    generation: u64,
    sequence: u64,
) -> bool {
    match value {
        Value::Object(object) => {
            value_object_digest(object).is_some_and(|(key, held)| {
                key.thread_id == thread && key.generation == generation && held < sequence
            }) || object
                .values()
                .any(|child| value_needs_digest_patch(child, thread, generation, sequence))
        }
        Value::Array(values) => values
            .iter()
            .any(|child| value_needs_digest_patch(child, thread, generation, sequence)),
        _ => false,
    }
}

pub fn patch_value_digests(
    value: &mut Value,
    thread: &str,
    generation: u64,
    sequence: u64,
) -> bool {
    match value {
        Value::Object(object) => {
            let changed = value_object_digest(object).is_some_and(|(key, held)| {
                key.thread_id == thread && key.generation == generation && held < sequence
            });
            if changed {
                object.insert("last_sequence".into(), Value::from(sequence));
            }
            object.values_mut().fold(changed, |changed, child| {
                patch_value_digests(child, thread, generation, sequence) | changed
            })
        }
        Value::Array(values) => values.iter_mut().fold(false, |changed, child| {
            patch_value_digests(child, thread, generation, sequence) | changed
        }),
        _ => false,
    }
}

#[cfg(test)]
#[path = "shared_json/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "shared_json/lifetime_tests.rs"]
mod lifetime_tests;
