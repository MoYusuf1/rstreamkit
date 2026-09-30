//! Reading a response body without trusting its size.

use std::{future::poll_fn, pin::Pin};

use futures_core::Stream;

#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub enum Capped<E> {
    /// The body passed the limit; it was not read any further.
    TooBig,
    Failed(E),
}

/// Hands a body to `each`, chunk by chunk as it arrives, stopping with `TooBig` as soon as it passes
/// `limit` bytes. `Content-Length` can be missing or wrong, and an endless stream where a playlist
/// or segment should be would otherwise be read forever.
pub async fn read_each<B: AsRef<[u8]>, E>(
    mut chunks: impl Stream<Item = Result<B, E>> + Unpin,
    limit: usize,
    mut each: impl FnMut(&[u8]),
) -> Result<(), Capped<E>> {
    let mut total = 0;
    while let Some(chunk) = poll_fn(|cx| Pin::new(&mut chunks).poll_next(cx)).await {
        let chunk = chunk.map_err(Capped::Failed)?;
        let chunk = chunk.as_ref();
        if chunk.len() > limit - total {
            return Err(Capped::TooBig);
        }
        total += chunk.len();
        each(chunk);
    }
    Ok(())
}

/// A whole body in memory, with the same cap. `expected` is what the server says it will send:
/// allocated once instead of doubled into place (which leaves the freed halves behind), and never
/// trusted beyond the limit.
pub async fn read_capped<B: AsRef<[u8]>, E>(
    chunks: impl Stream<Item = Result<B, E>> + Unpin,
    limit: usize,
    expected: usize,
) -> Result<Vec<u8>, Capped<E>> {
    let mut body = Vec::with_capacity(expected.min(limit));
    read_each(chunks, limit, |chunk| body.extend_from_slice(chunk)).await?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use std::{
        pin::pin,
        task::{Context, Poll, Waker},
    };

    use super::*;

    /// A stream that is always ready, made from an iterator.
    struct Chunks<I>(I);

    impl<I: Iterator + Unpin> Stream for Chunks<I> {
        type Item = I::Item;

        fn poll_next(mut self: Pin<&mut Self>, _: &mut Context) -> Poll<Option<I::Item>> {
            Poll::Ready(self.0.next())
        }
    }

    fn block_on<T>(f: impl Future<Output = T>) -> T {
        let mut f = pin!(f);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    #[test]
    fn an_endless_body_is_refused_after_the_limit_not_read_forever() {
        let mut pulled = 0;
        // "Endless" as far as a 4 MB cap goes; bounded so that a missing cap fails instead of hanging.
        let endless = Chunks(
            std::iter::repeat_with(|| {
                pulled += 1;
                Ok::<_, ()>(vec![0u8; 1 << 20])
            })
            .take(64),
        );
        assert_eq!(
            block_on(read_capped(endless, 4 << 20, 0)),
            Err(Capped::TooBig)
        );
        assert_eq!(
            pulled, 5,
            "four MB fit, the fifth chunk is where it stopped"
        );
    }

    #[test]
    fn a_body_exactly_at_the_limit_is_kept_and_one_byte_more_is_not() {
        let chunks = |sizes: &[usize]| {
            Chunks(
                sizes
                    .iter()
                    .map(|&n| Ok::<_, ()>(vec![7u8; n]))
                    .collect::<Vec<_>>()
                    .into_iter(),
            )
        };
        let body = block_on(read_capped(chunks(&[6, 4]), 10, 10)).unwrap();
        assert_eq!(body, [7; 10]);
        assert_eq!(
            block_on(read_capped(chunks(&[6, 5]), 10, 11)),
            Err(Capped::TooBig)
        );
        assert_eq!(block_on(read_capped(chunks(&[]), 10, 0)), Ok(vec![]));
    }

    #[test]
    fn a_failing_download_is_reported_as_that() {
        let broken = Chunks(vec![Ok(vec![1u8]), Err("connection reset")].into_iter());
        assert_eq!(
            block_on(read_capped(broken, 10, 0)),
            Err(Capped::Failed("connection reset"))
        );
    }
}
