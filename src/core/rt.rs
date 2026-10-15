//! One shared Tokio runtime for async networking (HTTP/2 via hyper,
//! WebSocket via tokio-tungstenite). GPUI keeps its own executor for UI work;
//! blocking callers use [`block_on`], async ones can [`spawn`] here.

use std::future::Future;
use std::sync::OnceLock;

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// The process-wide multi-threaded runtime, created on first use.
pub fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("rivet-net")
            .enable_all()
            .build()
            .expect("failed to start the networking runtime")
    })
}

/// Runs `future` to completion from synchronous code (never call from inside
/// a Tokio task — use `.await` there).
pub fn block_on<F: Future>(future: F) -> F::Output {
    runtime().block_on(future)
}

/// Spawns `future` on the shared runtime.
pub fn spawn<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    runtime().spawn(future)
}

#[cfg(test)]
mod tests {
    #[test]
    fn runs_futures_and_tasks() {
        assert_eq!(super::block_on(async { 40 + 2 }), 42);
        let handle = super::spawn(async { "ok" });
        assert_eq!(super::block_on(handle).unwrap(), "ok");
    }
}
