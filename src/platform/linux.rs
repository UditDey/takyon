use std::pin::Pin;
use std::future::Future;
use std::time::Duration;
use std::task::{Context, Poll};

use io_uring::{
    opcode,
    squeue,
    IoUring,
    Probe,
    types::Timespec
};

use nohash::{IntSet, IntMap};

use crate::{
    RUNTIME,
    runtime::TaskId,
    error::InitError
};

type IoKey = u32;

pub struct Platform {
    ring: IoUring,
    io_key_counter: IoKey,
    submitted_timeouts: IntMap<IoKey, TaskId>,
    completed_timeouts: IntSet<IoKey>,
    timespec_store: Vec<Box<Timespec>>
}

impl Platform {
    pub fn new() -> Result<Self, InitError> {
        Ok(Self {
            ring: new_io_uring()?,
            io_key_counter: 0,
            submitted_timeouts: IntMap::default(),
            completed_timeouts: IntSet::default(),
            timespec_store: Vec::new()
        })
    }

    pub fn sleep_fut(&self, dur: Duration) -> impl Future<Output = ()> {
        enum FutState {
            NotSubmitted(Duration),
            Submitted(IoKey),
            Done
        }

        struct SleepFut(FutState);

        impl Future for SleepFut {
            type Output = ();

            fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                match self.0 {
                    // Timeout not submitted yet
                    FutState::NotSubmitted(dur) => RUNTIME.with_borrow_mut(|rt| {
                        let key = rt.plat.new_io_key();
                        self.0 = FutState::Submitted(key);

                        rt.plat.submitted_timeouts.insert(key, rt.current_task);

                        let timespec = Box::new(Timespec::from(dur));
                        let sqe = opcode::Timeout::new(timespec.as_ref())
                            .build()
                            .user_data(key as u64);

                        rt.plat.timespec_store.push(timespec);
                        rt.plat.submit_sqe(sqe);

                        Poll::Pending
                    }),

                    // Timeout submitted, query it
                    FutState::Submitted(key) => RUNTIME.with_borrow_mut(|rt| {
                        if rt.plat.completed_timeouts.remove(&key) {
                            self.0 = FutState::Done;
                            Poll::Ready(())
                        }
                        else {
                            Poll::Pending
                        }
                    }),

                    FutState::Done => panic!("SleepFut polled even after completing")
                }
            }
        }

        impl Drop for SleepFut {
            fn drop(&mut self) {
                if let FutState::Submitted(key) = &self.0 {
                    RUNTIME.with_borrow_mut(|rt| {
                        if rt.plat.submitted_timeouts.remove(key).is_none() {
                            panic!("Key not found in submitted timeouts while dropping");
                        }
                
                        let sqe = opcode::TimeoutRemove::new(*key as u64).build();
                        rt.plat.submit_sqe(sqe);
                    });
                }
            }
        }

        SleepFut(FutState::NotSubmitted(dur))
    }

    pub fn wait_for_io(&mut self, wakeups: &mut Vec<TaskId>) {
        self.ring
            .submit_and_wait(1)
            .expect("Failed to submit io_uring");

        self.clear_stores();

        for cqe in self.ring.completion() {
            let key = IoKey::from(cqe.user_data() as u32);

            // Is this a timeout?
            if let Some(task_id) = self.submitted_timeouts.remove(&key) {
                self.completed_timeouts.insert(key);
                wakeups.push(task_id);
            }
        }
    }

    pub fn reset(&mut self) {
        // To get rid of pending IO we drop the current `io_uring` and
        // replace with a new one. We also don't unwrap `InitErrors` here
        // because since `init()` has already been succesfully called
        // once at this point we can be confident that creating a new
        // `io_uring` will be successful
        self.ring = new_io_uring().unwrap();

        self.io_key_counter = 0;
        self.submitted_timeouts = IntMap::default();
        self.completed_timeouts = IntSet::default();
        self.timespec_store = Vec::new();
    }

    fn new_io_key(&mut self) -> IoKey {
        let key = self.io_key_counter;
        self.io_key_counter = key.wrapping_add(1);

        key
    }

    fn submit_sqe(&mut self, sqe: squeue::Entry) {
        loop {
            // Try and push the sqe
            let res = unsafe {
                self.ring
                    .submission()
                    .push(&sqe)
            };

            match res {
                // Push successful, return
                Ok(()) => return,
                
                // No space left in submission queue
                // Submit it to make space and try again
                Err(_) => {
                    self.ring
                        .submit()
                        .expect("Failed to submit io_uring");

                    // Stores not needed after submission (submit_stable feature)
                    self.clear_stores();
                }
            }
        }
    }

    fn clear_stores(&mut self) {
        self.timespec_store.clear();
    }
}

fn new_io_uring() -> Result<IoUring, InitError> {
    let ring = IoUring::new(128).map_err(|err| InitError::IoUringCreationFailed(err))?;

    // Check required features
    if !ring.params().is_feature_submit_stable() {
        return Err(InitError::IoUringFeatureNotPresent("submit_stable"));
    }

    if !ring.params().is_feature_nodrop() {
        return Err(InitError::IoUringFeatureNotPresent("no_drop"));
    }

    // Probe supported opcodes
    let mut probe = Probe::new();

    ring.submitter()
        .register_probe(&mut probe)
        .map_err(|err| InitError::IoUringProbeFailed(err))?;

    // Check required opcodes
    let req_opcodes = [
        ("Timeout", opcode::Timeout::CODE)
    ];

    for (name, code) in req_opcodes {
        if !probe.is_supported(code) {
            return Err(InitError::IoUringOpcodeUnsupported(name));
        }
    }

    Ok(ring)
}