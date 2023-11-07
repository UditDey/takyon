use std::mem;
use std::any::Any;
use std::pin::Pin;
use std::future::Future;

use nohash::IntMap;

use crate::{
    JoinHandle,
    platform::Platform,
    error::InitError
};

pub type TaskId = u32;

type Task = Pin<Box<dyn Future<Output = Box<dyn Any>>>>;

pub enum WokenTask {
    Root,
    Child(Task)
}

struct JoinHandleInfo {
    result: Option<Box<dyn Any>>,
    waiting_task: Option<TaskId>
}

pub struct Runtime {
    task_id_counter: TaskId,
    
    pub current_task: TaskId,
    tasks: IntMap<TaskId, Task>,
    join_handles: IntMap<TaskId, JoinHandleInfo>,
    task_wakeups: Vec<TaskId>,
    
    pub plat: Platform,
}

impl Runtime {
    pub fn new() -> Result<Self, InitError> {
        let plat = Platform::new()?;

        Ok(Self {
            task_id_counter: 1, // Task IDs start from 1 since 0 is reserved for root task
            current_task: 0,
            tasks: IntMap::default(),
            join_handles: IntMap::default(),
            task_wakeups: vec![0], // We always start with the root task already woken up
            plat
        })
    }

    fn new_task_id(&mut self) -> TaskId {
        let id = self.task_id_counter;
        self.task_id_counter = id.wrapping_add(1);

        // TaskId 0 is reserved for the root task
        if self.task_id_counter == 0 {
            self.task_id_counter = 1;
        }

        id
    }

    pub fn reset(&mut self) -> IntMap<TaskId, Task> {
        self.task_id_counter = 1;
        self.current_task = 0;
        self.join_handles = IntMap::default();
        self.task_wakeups = vec![0];
        self.plat.reset();

        // Instead of simply clearing the task list, we replace it and
        // give ownership of the tasks to `run()`. This prevents double
        // borrows of the thread local runtime since task destructors
        // may also borrow it (to cancel IO for example).
        // This way the tasks will be dropped inside `run()` where
        // each task can get exclusive access to the Runtime
        mem::replace(&mut self.tasks, IntMap::default())
    }

    pub fn wait_for_io(&mut self) {
        self.plat.wait_for_io(&mut self.task_wakeups)
    }
}


// Functions related to tasks
//
// These functions are used by `run()` and `spawn()`
//
// How `run()` operates here is:
// 1) Call `get_woken_task()` to pop a TaskId off the wakeup list and get the corresponding task
//    This also marks the ID of the retrieved task as the "current task", which will then be used,
//    for example, to correlate what IO was submitted by what task
// 
// 2) Run the retrieved task. If it returns Poll::Pending, place it back into the task list with
//    `return_task()`, if it returns Poll::Ready(result), store its result into the task's join
//    handle with `task_finished()`.
//    `task_finished()` will also place the ID of the task waiting on the current task's join
//    handle into the wakeup list
//
// 3) The process repeats until `get_woken_task()` returns `None`, indicating that no more woken
//    up tasks are left, and we must wait for IO to wake up more tasks
//
// The reason Runtime gives ownership of woken up tasks to `run()` instead of directly running
// them is to prevent double borrows of the thread local Runtime (which would in fact result in
// iterator invalidation since a running task can spawn more tasks into the task list, which can
// cause reallocation, and would break a simple iterator)
impl Runtime {
    pub fn spawn<F: Future + 'static>(&mut self, task: F) -> JoinHandle<F::Output> {
        let id = self.new_task_id();

        // Wrap the task to erase its output type
        let wrapped_task = async {
            let res: Box<dyn Any> = Box::new(task.await);
            res
        };

        // Place the task in the task list, create it's join handle, and also place in
        // the wakeup list to give it an initial poll
        self.tasks.insert(id, Box::pin(wrapped_task));
        self.join_handles.insert(id, JoinHandleInfo { result: None, waiting_task: None });
        self.task_wakeups.push(id);

        JoinHandle::new(id)
    }

    pub fn get_woken_task(&mut self) -> Option<WokenTask> {
        loop {
            let id = self.task_wakeups.pop()?;
            self.current_task = id;

            if id == 0 {
                return Some(WokenTask::Root);
            }
            else {
                // Note that its possible for a woken up task id to not be present in the list
                // This happens when a task has multiple wakeups, but completes before processing
                // all of them, in that case this `id` will no longer be found in the task list
                // and we should continue with the next id
                match self.tasks.remove(&id) {
                    Some(task) => return Some(WokenTask::Child(task)),
                    None => continue
                }
            }
        }
    }

    pub fn task_finished(&mut self, res: Box<dyn Any>) {
        match self.join_handles.get_mut(&self.current_task) {
            // Write result into join handle and wake up it's waiting task, if any
            Some(handle) => {
                handle.result = Some(res);

                if let Some(id) = handle.waiting_task {
                    self.task_wakeups.push(id);
                }
            },

            // Join handle dropped, discard result
            None => ()
        }
    }

    pub fn return_task(&mut self, task: Task) {
        self.tasks.insert(self.current_task, task);
    }
}

// Functions related to join handles
//
// How a `JoinHandle` operates here is:
// 1) When a task is spawned, a corresponding `JoinHandleInfo` is created, into which the task's
//    result will be stored when its finished. When polled for the first time, the task's
//    `JoinHandle` will use `pop_join_handle_result()` to try retrieve it's result and remove the
//    `JoinHandleInfo` from the list if its done
// 
// 2) If the task hasn't completed yet then the `JoinHandle` will use `register_join_handle_wakeup()`
//    to register the `current_task` to be woken up when the other task has completed. On subsequent
//    polls, `JoinHandle` will repeatedly use `pop_join_handle_result()` to try and get the result
//
// 3) While dropping, a `JoinHandle` uses `drop_join_handle()` to remove it's `JoinHandleInfo` from
//    the list, after this when the corresponding task finishes it's result is discarded
impl Runtime {
    pub fn pop_join_handle_result(&mut self, id: TaskId) -> Option<Box<dyn Any>> {
        let info = self.join_handles.remove(&id).expect("Join handle info not found");

        if let Some(res) = info.result {
            Some(res)
        }
        else {
            self.join_handles.insert(id, info);
            None
        }
    }

    pub fn register_join_handle_wakeup(&mut self, id: TaskId) {
        let info = self.join_handles.get_mut(&id).expect("Join handle info not found");
        info.waiting_task = Some(self.current_task);
    }

    pub fn drop_join_handle(&mut self, id: TaskId) {
        self.join_handles.remove(&id);
    }
}