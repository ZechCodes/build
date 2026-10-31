//! One application stream per execution opening, with bounded predecessor state.
//! Stream handles are locators; callers authenticate the opening before use.
use std::sync::Mutex;

pub trait RetireBinding: Clone {
    fn retire(&self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamError {
    Required,
    Replaced,
    Consumed,
}

struct Issued<T> {
    handle: String,
    consumed: bool,
    binding: Option<T>,
    predecessor: Option<T>,
}

/// Keep the last issued handle even after delete/disconnect. Otherwise an old
/// first POST could be mistaken for a new first POST and replace a later stream.
pub struct StreamSlot<T>(Mutex<Option<Issued<T>>>);

impl<T> Default for StreamSlot<T> {
    fn default() -> Self {
        Self(Mutex::new(None))
    }
}

impl<T: RetireBinding> StreamSlot<T> {
    pub fn replace(&self, expected: Option<&str>) -> Result<String, StreamError> {
        let mut state = self.0.lock().unwrap();
        if state.as_ref().map(|value| value.handle.as_str()) != expected {
            return Err(StreamError::Replaced);
        }
        let predecessor = state
            .as_mut()
            .and_then(|value| value.binding.take().or_else(|| value.predecessor.take()));
        if let Some(binding) = &predecessor {
            binding.retire();
        }
        let handle = uuid::Uuid::new_v4().to_string();
        *state = Some(Issued {
            handle: handle.clone(),
            consumed: false,
            binding: None,
            predecessor,
        });
        Ok(handle)
    }

    /// The closure installs a new shared EventBinding while this handle remains
    /// current. It must not await or acquire an application/domain lock.
    pub fn bind(
        &self,
        handle: &str,
        make: impl FnOnce(Option<&T>) -> Result<T, StreamError>,
    ) -> Result<T, StreamError> {
        let mut state = self.0.lock().unwrap();
        let issued = current(&mut state, handle)?;
        if issued.consumed {
            return Err(StreamError::Consumed);
        }
        let binding = make(issued.predecessor.as_ref())?;
        issued.consumed = true;
        issued.binding = Some(binding.clone());
        issued.predecessor = None;
        Ok(binding)
    }

    pub fn binding(&self, expected: Option<&str>) -> Result<T, StreamError> {
        let handle = expected.ok_or(StreamError::Required)?;
        let mut state = self.0.lock().unwrap();
        current(&mut state, handle)?
            .binding
            .clone()
            .ok_or(StreamError::Replaced)
    }

    pub fn delete(&self, handle: &str) -> Result<(), StreamError> {
        let mut state = self.0.lock().unwrap();
        retire_issued(current(&mut state, handle)?);
        Ok(())
    }

    /// Opening teardown: retire whatever is current, keeping its tombstone.
    pub fn close(&self) {
        if let Some(issued) = self.0.lock().unwrap().as_mut() {
            retire_issued(issued);
        }
    }

    pub fn release(&self, handle: &str) {
        let _ = self.delete(handle);
    }
}

fn retire_issued<T: RetireBinding>(issued: &mut Issued<T>) {
    // A deleted unused handle must never be usable by a delayed GET.
    issued.consumed = true;
    if let Some(binding) = issued.binding.take() {
        binding.retire();
        issued.predecessor = Some(binding);
    }
}

fn current<'a, T>(
    state: &'a mut Option<Issued<T>>,
    handle: &str,
) -> Result<&'a mut Issued<T>, StreamError> {
    state
        .as_mut()
        .filter(|value| value.handle == handle)
        .ok_or(StreamError::Replaced)
}
