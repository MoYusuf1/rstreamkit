use std::{
    cell::{Cell, RefCell},
    future::Future,
    task::Poll,
};

// Poll cancellation before resuming any suspended playback operation. Dropping the inner
// future releases body readers, event listeners and the MediaSource immediately.
pub(crate) async fn cancellable<T>(
    stop: &Cell<bool>,
    wake: &RefCell<Option<std::task::Waker>>,
    future: impl Future<Output = T>,
) -> Option<T> {
    let mut future = std::pin::pin!(future);
    std::future::poll_fn(|cx| {
        if stop.get() {
            return Poll::Ready(None);
        }
        *wake.borrow_mut() = Some(cx.waker().clone());
        future.as_mut().poll(cx).map(Some)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        pin::pin,
        rc::Rc,
        task::{Context, Waker},
    };

    #[test]
    fn cancellation_drops_pending_work_without_resuming_it() {
        struct Pending(Rc<Cell<bool>>);
        impl Future for Pending {
            type Output = ();
            fn poll(self: std::pin::Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
                Poll::Pending
            }
        }
        impl Drop for Pending {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        let dropped = Rc::new(Cell::new(false));
        let stop = Cell::new(false);
        let wake = RefCell::new(None);
        let mut future = pin!(cancellable(&stop, &wake, Pending(dropped.clone())));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert!(wake.borrow().is_some());
        stop.set(true);
        wake.borrow_mut().take().unwrap().wake();
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(None));
        assert!(dropped.get());
    }

    #[test]
    fn completed_work_returns_its_value() {
        let stop = Cell::new(false);
        let wake = RefCell::new(None);
        let mut future = pin!(cancellable(&stop, &wake, std::future::ready(42)));
        assert_eq!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Some(42))
        );
    }
}
