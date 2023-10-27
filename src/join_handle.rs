use std::pin::Pin;
use std::cell::Cell;
use std::future::Future;
use std::marker::PhantomData;
use std::task::{Context, Poll};

use crate::{RUNTIME, runtime::TaskId};

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