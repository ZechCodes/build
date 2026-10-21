//! Test-first adapter for required service futures that outlive a request.
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

#[cfg(test)]
mod tests;

pub(crate) struct ServiceFuture<F> {
    future: Pin<Box<F>>,
}

impl<F: Future> ServiceFuture<F> {
    pub(crate) fn queued(future: F) -> Self {
        // Deliberately inert until the real Pending/drop controls prove that
        // forwarding a future alone loses its required originating write.
        Self {
            future: Box::pin(future),
        }
    }
}

impl<F: Future> Future for ServiceFuture<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().future.as_mut().poll(cx)
    }
}
