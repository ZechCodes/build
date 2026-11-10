//! Borrow selected fields from either accepted JSON fragment shape.
use super::SharedJson;
use serde_json::Value;

#[derive(Clone, Copy)]
pub(super) enum JsonRef<'a> {
    Shared(&'a SharedJson),
    Value(&'a Value),
}

impl<'a> JsonRef<'a> {
    pub(super) fn field(self, key: &str) -> Self {
        match self {
            Self::Value(value) => Self::Value(&value[key]),
            Self::Shared(shared) => match shared.leaf() {
                Some(value) => Self::Value(&value[key]),
                None => shared
                    .fields()
                    .and_then(|fields| fields.get(key))
                    .map(Self::Shared)
                    .unwrap_or(Self::Value(&Value::Null)),
            },
        }
    }

    pub(super) fn string(self) -> Option<&'a str> {
        match self {
            Self::Value(value) => value.as_str(),
            Self::Shared(shared) => shared.leaf().and_then(Value::as_str),
        }
    }

    pub(super) fn value(self) -> Value {
        match self {
            Self::Value(value) => value.clone(),
            Self::Shared(shared) => shared.materialize(),
        }
    }

    pub(super) fn rows(self) -> Box<dyn Iterator<Item = Self> + 'a> {
        match self {
            Self::Value(value) => Box::new(value.as_array().into_iter().flatten().map(Self::Value)),
            Self::Shared(shared) => match shared.leaf() {
                Some(value) => Self::Value(value).rows(),
                None => Box::new(shared.elements().into_iter().flatten().map(Self::Shared)),
            },
        }
    }
}
