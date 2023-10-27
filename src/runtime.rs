use std::mem;
use std::any::Any;
use std::pin::Pin;
use std::future::Future;
use std::time::Duration;

use nohash::IntMap;

use crate::{
    JoinHandle,
    platform::Platform,
    error::InitError
};

pub type IoKey = u32;
pub type TaskId = u32;

type Task = Pin<Box<dyn Future<Output = Box<dyn Any>>>>;

pub enum WokenTask {
    RootTask,
    ChildTask(Task)
}

struct JoinHandleInfo {
    result: Option<Box<dyn Any>>,
    waiting_task: Option<TaskId>
}

pub struct Runtime {
    io_key_counter: IoKey,
    task_id_counter: TaskId,
    
    current_task: TaskId,
    tasks: IntMap<TaskId, Task>,
    join_handles: IntMap<TaskId, JoinHandleInfo>,
    task_wakeups: Vec<TaskId>,
    
    plat: Platform,
}

impl Runtime {
    pub fn new() -> Result<Self, InitError> {
        let plat = Platform::new()?;

        Ok(Self {
            io_key_counter: 0,
            task_id_counter: 1,
            current_task: 0,
            tasks: IntMap::default(),
            join_handles: IntMap::default(),
            task_wakeups: vec![0], // We always start with the root task already woken up
            plat
        })
    }

    fn new_io_key(&mut self) -> IoKey {
        let key = self.io_key_counter;
        self.io_key_counter = key.wrapping_add(1);

        key
    }

    fn new_task_id(&mut self) -> TaskId {
        let id = self.task_id_counter;
        self.task_id_counter += id.wrapping_add(1);

        // TaskId 0 is reserved for the root task
        if self.task_id_counter == 0 {
            self.task_id_counter = 1;
        }

        id
    }

    pub fn reset(&mut self) -> IntMap<TaskId, Task> {
        self.io_key_counter = 0;
        self.task_id_counter = 1;
        self.current_task = 0;
        self.join_handles = IntMap::default();
        self.task_wakeups = vec![0];
        self.plat.reset();

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
        let id = self.task_wakeups.pop()?;
        self.current_task = id;

        if id == 0 {
            Some(WokenTask::RootTask)
        }
        else {
            let task = self.tasks.remove(&id).expect("Woken up task not found in task list");
            Some(WokenTask::ChildTask(task))
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


// Functions related to timeouts
impl Runtime {
    pub fn push_timeout(&mut self, dur: Duration) -> IoKey {
        let key = self.new_io_key();
        self.plat.push_timeout(self.current_task, key, dur);

        key
    }

    pub fn cancel_timeout(&mut self, key: IoKey) {
        self.plat.cancel_timeout(key)
    }

    pub fn pop_timeout(&mut self, key: IoKey) -> bool {
        self.plat.pop_timeout(key)
    }
}