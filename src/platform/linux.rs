use std::time::Duration;

use io_uring::{
    opcode,
    squeue,
    IoUring,
    Probe,
    types::Timespec
};

use nohash::{IntSet, IntMap};

use crate::{
    error::InitError,
    runtime::{IoKey, TaskId},
};

pub struct Platform {
    ring: IoUring,
    submitted_timeouts: IntMap<IoKey, TaskId>,
    completed_timeouts: IntSet<IoKey>,
    timespec_store: Vec<Box<Timespec>>
}

impl Platform {
    pub fn new() -> Result<Self, InitError> {
        Ok(Self {
            ring: new_io_uring()?,
            submitted_timeouts: IntMap::default(),
            completed_timeouts: IntSet::default(),
            timespec_store: Vec::new()
        })
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


    // Timeouts
    pub fn push_timeout(&mut self, task_id: TaskId, key: IoKey, dur: Duration) {
        self.submitted_timeouts.insert(key, task_id);

        let timespec = Box::new(Timespec::from(dur));

        let sqe = opcode::Timeout::new(timespec.as_ref())
            .build()
            .user_data(key as u64);

        self.timespec_store.push(timespec);
        self.submit_sqe(sqe);
    }

    pub fn cancel_timeout(&mut self, key: IoKey) {
        if self.submitted_timeouts.remove(&key).is_none() {
            panic!("Specified key not found in submitted timeouts, maybe wrong key was used?");
        }

        let sqe = opcode::TimeoutRemove::new(key as u64).build();
        self.submit_sqe(sqe);
    }

    pub fn pop_timeout(&mut self, key: IoKey) -> bool {
        self.completed_timeouts.remove(&key)
    }


    pub fn wait_for_io(&mut self, wakeups: &mut Vec<TaskId>) {
        self.ring
            .submit_and_wait(1)
            .expect("Failed to submit io_uring");

        self.clear_stores();

        for cqe in self.ring.completion() {
            let key = cqe.user_data() as IoKey;

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

        self.submitted_timeouts = IntMap::default();
        self.completed_timeouts = IntSet::default();
        self.timespec_store = Vec::new();
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