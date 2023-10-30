use std::pin::Pin;
use std::cell::Cell;
use std::future::Future;
use std::marker::PhantomData;
use std::task::{Context, Poll};

use crate::{RUNTIME, runtime::TaskId};

/// A handle to a spawned task
/// 
/// This is a future that can be `await`ed to wait for a task to complete and
/// obtain its result. However this handle does not need to be awaited for the
/// task to run. If the handle is dropped then the task will continue to run
/// but it's result will be lost
/// 
/// # Safety
/// A [`JoinHandle`] is valid only inside the [`run()`](crate::run) call that
/// it was produced within. Please don't try to use a join handle from one
/// [`run()`](crate::run) call in another
/// 
/// # Examples
/// ```
/// takyon::run(async {
///     // Spawn a task to do some work
///     let handle = takyon::spawn(async {
///         let result = do_something().await;
///         println!("Something done");
///         result
///     });
/// 
///     // Meanwhile do some other work
///     do_something_else().await;
///     println!("Something else done");
/// 
///     // Wait for the task to finish
///     let task_result = handle.await;
///     println!("{task_result}");
/// });
/// ```
pub struct JoinHandle<T: 'static> {
    id: TaskId,
    registered: Cell<bool>,
    phantom: PhantomData<T>
}

impl<T> JoinHandle<T> {
    pub(crate) fn new(id: TaskId) -> Self {
        Self {
            id,
            registered: Cell::new(false),
            phantom: PhantomData
        }
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        let res = RUNTIME.with_borrow_mut(|rt| rt.pop_join_handle_result(self.id));

        match res {
            Some(res) => {
                let res = *res.downcast::<T>().expect("Type error when downcasting task result");
                Poll::Ready(res)
            },
            None => {
                if !self.registered.get() {
                    RUNTIME.with_borrow_mut(|rt| rt.register_join_handle_wakeup(self.id));
                    self.registered.set(true);
                }
                
                Poll::Pending
            }
        }
    }
}

impl<T> Drop for JoinHandle<T> {
    fn drop(&mut self) {
        RUNTIME.with_borrow_mut(|rt| rt.drop_join_handle(self.id));
    }
}