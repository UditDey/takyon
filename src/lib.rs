mod error;
mod platform;
mod executor;

pub mod time;
pub mod fs;

use std::ptr;
use std::pin::pin;
use std::future::Future;
use std::task::{Poll, Context, Waker, RawWaker, RawWakerVTable};

pub use error::Error;
pub use executor::Executor;

static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(|_| panic!(), |_| (), |_| (), |_| ());

pub fn run_raw<F, B, O>(builder: B) -> Result<O, Error>
where
    F: Future<Output = O>,
    B: Fn(*const Executor) -> F
{
    let waker = unsafe { Waker::from_raw(RawWaker::new(ptr::null(), &WAKER_VTABLE)) };
    let mut cx = Context::from_waker(&waker);

    let exec = Executor::new()?;
    let mut fut = pin!(builder(&exec));

    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Pending => exec.wait_for_events()?,
            Poll::Ready(val) => return Ok(val)
        }
    }
}

#[macro_export]
macro_rules! run {
    (async $(move $(@$move:tt)?)? |$arg:tt $(: $ArgTy:ty)? $(,)?| $(-> $Ret:ty)? $body:block) => {
        $crate::run_raw(|arg: *const $crate::Executor| async move {
            // Wacky bit ahead:
            // This macro is needed because right now on stable rust, we can't convince the compiler
            // that the async block returned by the closure does not outlive the Executor reference
            // that is passed to it So we pass a raw pointer instead of a reference and unsafely convert
            // it to a `&Executor`.
            // 
            // This bit can help maintain soundness despite the unsafe deref by tying the lifetime of the
            // `&Executor` to a local variable, assuring the compiler that the async block has the same
            // lifetime as the reference and does not outlive it
            let local = [];

            let arg: &$crate::Executor = if true {
                unsafe { &*arg }
            }
            else {
                &local[0]
            };

            let $arg $(: $ArgTy)? = arg;
            
            $body
        })
    };
}