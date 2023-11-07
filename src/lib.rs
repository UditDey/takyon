mod error;
mod runtime;
mod platform;
mod join_handle;

pub mod net;
pub mod time;
pub mod util;

pub use error::InitError;
pub use join_handle::JoinHandle;

use std::ptr;
use std::pin::pin;
use std::future::Future;
use std::cell::{Cell, RefCell};
use std::task::{Poll, Context, Waker, RawWaker, RawWakerVTable};

use runtime::{Runtime, WokenTask};

thread_local! {
    // The lazy initializer panics, therefore enforcing that the runtime is initialized only using init()
    // This works because init() uses .set() which does not run the lazy initializer
    pub(crate) static RUNTIME: RefCell<Runtime> = panic!("takyon::init() has not been called on this thread!");

    pub(crate) static RUNNING: Cell<bool> = const { Cell::new(false) };
}

static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(|_| panic!(), |_| (), |_| (), |_| ());

/// Initializes the thread-local runtime
/// 
/// This must be called at least once before calling [`run()`] on a thread
pub fn init() -> Result<(), InitError> {
    RUNTIME.set(Runtime::new()?);
    Ok(())
}

/// Runs a future on the current thread, blocking it whenever waiting for IO
/// 
/// The passed future will be considered the "root task". The root task can
/// use [`spawn()`] to spawn child tasks. The function returns the root task's
/// result as soon as it has finished, and all pending child tasks will be dropped.
/// 
/// Use the child tasks' [`JoinHandle`]s if you want to wait for them to complete
/// before returning. Remember to call [`init()`] atleast once on a thread before
/// using [`run()`]
/// 
/// # Examples
/// ```
/// use takyon::time::sleep_secs;
/// 
/// // Initialize the thread-local runtime
/// takyon::init()?;
/// 
/// // Run a future
/// let result = takyon::run(async {
///     sleep_secs(1).await;
///     println!("1 second passed");
/// 
///     sleep_secs(1).await;
///     println!("2 seconds passed");
/// 
///     let result = do_something().await;
/// 
///     result
/// });
/// 
/// // Use the result returned by the future
/// println!("{result}");
/// ```
pub fn run<F: Future>(root_task: F) -> F::Output {
    RUNNING.set(true);

    let waker = unsafe { Waker::from_raw(RawWaker::new(ptr::null(), &WAKER_VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    
    let mut root_task = pin!(root_task);

    loop {
        // Poll all woken up tasks
        loop {
            let task = RUNTIME.with_borrow_mut(|rt| rt.get_woken_task());

            match task {
                // Root task woken up
                Some(WokenTask::Root) => {
                    let poll = root_task.as_mut().poll(&mut cx);

                    // Root task finished, reset runtime and return
                    if let Poll::Ready(res) = poll {
                        RUNTIME.with_borrow_mut(|rt| rt.reset());
                        RUNNING.set(false);
                        return res;
                    }
                },
    
                // Child task woken up
                Some(WokenTask::Child(mut task)) => {
                    let poll = task.as_mut().poll(&mut cx);

                    match poll {
                        // Child task pending, return it into task list
                        Poll::Pending => RUNTIME.with_borrow_mut(|rt| rt.return_task(task)),
                        
                        // Child task finished with result
                        Poll::Ready(res) => RUNTIME.with_borrow_mut(|rt| rt.task_finished(res))
                    }
                },

                // No more woken tasks left
                None => break
            }
        }

        // Wait for IO events to wake up more tasks
        RUNTIME.with_borrow_mut(|rt| rt.wait_for_io());
    }
}

/// Spawns a new task and returns it's [`JoinHandle`]
/// 
/// The new task immediately runs concurrently with the current task without needing to
/// `await` it. The returned [`JoinHandle`] can be used to wait for the task to finish
/// 
/// See the [`JoinHandle`] docs for an example of using this function
/// 
/// # Panics
/// This can only be used within a [`run()`] call and will panic if used outside it
pub fn spawn<F: Future + 'static>(task: F) -> JoinHandle<F::Output> {
    if !RUNNING.get() {
        panic!("takyon::spawn() called outside of a takyon::run() call!")
    }

    RUNTIME.with_borrow_mut(|rt| rt.spawn(task))
}