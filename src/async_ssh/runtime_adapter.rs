// Runtime adapter to handle both custom executor and tokio contexts

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Detect if we're in a tokio runtime context
pub fn in_tokio_context() -> bool {
    #[cfg(feature = "tokio")]
    {
        tokio::runtime::Handle::try_current().is_ok()
    }
    #[cfg(not(feature = "tokio"))]
    {
        false
    }
}

/// A future that can run in both tokio and custom executor contexts
pub struct RuntimeAdaptedFuture<F> {
    inner: F,
}

impl<F> RuntimeAdaptedFuture<F> 
where 
    F: Future,
{
    pub fn new(future: F) -> Self {
        Self { inner: future }
    }
}

impl<F> Future for RuntimeAdaptedFuture<F>
where
    F: Future + Unpin,
{
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // In tokio context, poll normally
        // In custom executor context, poll normally
        // The key is not to create conflicting reactor registrations
        Pin::new(&mut self.inner).poll(cx)
    }
}

/// Spawn a future in the appropriate runtime
pub fn spawn_adapted<F>(future: F) 
where
    F: Future<Output = ()> + Send + 'static,
{
    if in_tokio_context() {
        #[cfg(feature = "tokio")]
        {
            tokio::spawn(future);
        }
    } else {
        crate::executor::spawn(future);
    }
}

/// Block on a future in the appropriate runtime  
pub fn block_on_adapted<F>(future: F) -> F::Output
where
    F: Future,
{
    if in_tokio_context() {
        #[cfg(feature = "tokio")]
        {
            // We're already in tokio context, can't block_on
            // This should not happen in properly designed async code
            panic!("Cannot block_on within tokio context - use .await instead");
        }
        #[cfg(not(feature = "tokio"))]
        {
            crate::executor::block_on(future).unwrap()
        }
    } else {
        crate::executor::block_on(future).unwrap()
    }
}