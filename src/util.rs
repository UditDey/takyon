//! Utility functions for working with futures
//! 
//! Provides similar functionality as the utility functions in the `futures_lite` crate

use std::pin::Pin;
use std::future::Future;
use std::task::{Context, Poll};

use pin_project_lite::pin_project;

pin_project! {
    struct Or<F1, F2> {
        #[pin]
        fut1: F1,

        #[pin]
        fut2: F2
    }
}

impl<T, F1, F2> Future for Or<F1, F2>
where
    F1: Future<Output = T>,
    F2: Future<Output = T>
{
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> std::task::Poll<Self::Output> {
        let this = self.project();

        if let Poll::Ready(t) = this.fut1.poll(cx) {
            return Poll::Ready(t);
        }

        if let Poll::Ready(t) = this.fut2.poll(cx) {
            return Poll::Ready(t);
        }
        
        Poll::Pending
    }
}

/// Returns the result of the future that completes first, preferring `fut1` if both are ready
pub async fn or<T, F1, F2>(fut1: F1, fut2: F2) -> T
where
    F1: Future<Output = T>,
    F2: Future<Output = T>
{
    Or { fut1, fut2 }.await
}

pin_project! {
    struct Zip<F1, F2>
    where
        F1: Future,
        F2: Future
    {
        #[pin] fut1: F1,
        out1: Option<F1::Output>,
        #[pin] fut2: F2,
        out2: Option<F2::Output>
    }
}

impl<F1, F2> Future for Zip<F1, F2>
where
    F1: Future,
    F2: Future
{
    type Output = (F1::Output, F2::Output);

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        if this.out1.is_none() {
            if let Poll::Ready(out) = this.fut1.poll(cx) {
                *this.out1 = Some(out);
            }
        }

        if this.out2.is_none() {
            if let Poll::Ready(out) = this.fut2.poll(cx) {
                *this.out2 = Some(out);
            }
        }

        match (this.out1.take(), this.out2.take()) {
            (Some(t1), Some(t2)) => Poll::Ready((t1, t2)),

            (out1, out2) => {
                *this.out1 = out1;
                *this.out2 = out2;
                Poll::Pending
            }
        }
    }
}

/// Joins two futures, concurrently waiting for both to complete
pub async fn zip<F1, F2>(fut1: F1, fut2: F2) -> (F1::Output, F2::Output)
where
    F1: Future,
    F2: Future,
{
    Zip {
        fut1,
        fut2,
        out1: None,
        out2: None,
    }.await
}

pin_project! {
    struct TryZip<F1, T1, F2, T2> {
        #[pin] fut1: F1,
        out1: Option<T1>,
        #[pin] fut2: F2,
        out2: Option<T2>
    }
}

impl<F1, T1, F2, T2, E> Future for TryZip<F1, T1, F2, T2>
where
    F1: Future<Output = Result<T1, E>>,
    F2: Future<Output = Result<T2, E>>
{
    type Output = Result<(T1, T2), E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        if this.out1.is_none() {
            if let Poll::Ready(out) = this.fut1.poll(cx) {
                match out {
                    Ok(t) => *this.out1 = Some(t),
                    Err(err) => return Poll::Ready(Err(err)),
                }
            }
        }

        if this.out2.is_none() {
            if let Poll::Ready(out) = this.fut2.poll(cx) {
                match out {
                    Ok(t) => *this.out2 = Some(t),
                    Err(err) => return Poll::Ready(Err(err)),
                }
            }
        }

        match (this.out1.take(), this.out2.take()) {
            (Some(t1), Some(t2)) => Poll::Ready(Ok((t1, t2))),

            (out1, out2) => {
                *this.out1 = out1;
                *this.out2 = out2;
                Poll::Pending
            }
        }
    }
}

/// Joins two fallible futures, concurrently waiting for both to complete or one of them to error
pub async fn try_zip<F1, T1, F2, T2, E>(fut1: F1, fut2: F2) -> Result<(T1, T2), E>
where
    F1: Future<Output = Result<T1, E>>,
    F2: Future<Output = Result<T2, E>>,
{
    TryZip {
        fut1,
        fut2,
        out1: None,
        out2: None,
    }.await
}