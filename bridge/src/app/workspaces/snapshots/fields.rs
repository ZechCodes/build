//! Borrow selection fields from mixed immutable JSON without materializing rows.
use crate::app::read_view::SharedJson;
use serde_json::Value;

#[derive(Clone, Copy)]
pub(super) enum Cursor<'a> {
    Shared(&'a SharedJson),
    Value(&'a Value),
}

impl<'a> Cursor<'a> {
    pub fn shared(value: &'a SharedJson) -> Self {
        Self::Shared(value)
    }

    #[cfg(test)]
    pub fn value(value: &'a Value) -> Self {
        Self::Value(value)
    }

    pub fn field(self, key: &str) -> Option<Self> {
        match self {
            Self::Value(value) => value.get(key).map(Self::Value),
            Self::Shared(node) => match node.leaf() {
                Some(value) => value.get(key).map(Self::Value),
                None => node.fields()?.get(key).map(Self::Shared),
            },
        }
    }

    pub fn as_str(self) -> Option<&'a str> {
        match self {
            Self::Value(value) => value.as_str(),
            Self::Shared(node) => node.leaf()?.as_str(),
        }
    }

    pub fn array(self) -> Elements<'a> {
        match self {
            Self::Value(value) => {
                Elements::Values(value.as_array().map(Vec::as_slice).unwrap_or(&[]).iter())
            }
            Self::Shared(node) => match node.leaf() {
                Some(value) => Self::Value(value).array(),
                None => Elements::Shared(node.elements().unwrap_or(&[]).iter()),
            },
        }
    }
}

pub(super) enum Elements<'a> {
    Shared(std::slice::Iter<'a, SharedJson>),
    Values(std::slice::Iter<'a, Value>),
}

impl<'a> Iterator for Elements<'a> {
    type Item = Cursor<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Shared(values) => values.next().map(Cursor::Shared),
            Self::Values(values) => values.next().map(Cursor::Value),
        }
    }
}
