use crate::PolySynth;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
};

pub(crate) const CAPACITY: usize = 128;

/// Serialized producers publish fully initialized atomic slots before tail.
/// Only the leased consumer advances head, and it does so after reading slots.
/// Producers never reclaim slots through reset: this prevents an in-flight
/// consumer from reading overwritten events, including during overflow recovery.
pub(crate) struct NoteQueue {
    writers: Mutex<()>,
    slots: [AtomicU32; CAPACITY],
    head: AtomicU64,
    tail: AtomicU64,
    reset_version: AtomicU64,
    reset_boundary: AtomicU64,
    claimed: AtomicBool,
}

impl Default for NoteQueue {
    fn default() -> Self {
        Self {
            writers: Mutex::new(()),
            slots: std::array::from_fn(|_| AtomicU32::new(0)),
            head: AtomicU64::new(0),
            tail: AtomicU64::new(0),
            reset_version: AtomicU64::new(0),
            reset_boundary: AtomicU64::new(0),
            claimed: AtomicBool::new(false),
        }
    }
}

impl NoteQueue {
    pub(crate) fn push(&self, note: u8, velocity: u8) -> Result<(), String> {
        let _writer = self.writers.lock().unwrap_or_else(|e| e.into_inner());
        let tail = self.tail.load(Ordering::SeqCst);
        if tail - self.head.load(Ordering::SeqCst) >= CAPACITY as u64 || tail == u64::MAX {
            self.reset_locked();
            return Err(
                "Note queue is full; all notes will be released at the next audio buffer".into(),
            );
        }
        self.slots[(tail % CAPACITY as u64) as usize].store(
            u32::from(note) | (u32::from(velocity) << 8),
            Ordering::SeqCst,
        );
        self.tail.store(tail + 1, Ordering::SeqCst);
        Ok(())
    }

    pub(crate) fn reset(&self) {
        let _writer = self.writers.lock().unwrap_or_else(|e| e.into_inner());
        self.reset_locked();
    }

    fn reset_locked(&self) {
        self.reset_version.fetch_add(1, Ordering::SeqCst);
        self.reset_boundary
            .store(self.tail.load(Ordering::SeqCst), Ordering::SeqCst);
        self.reset_version.fetch_add(1, Ordering::SeqCst);
    }
}

pub(crate) struct ConsumerLease {
    queue: Arc<NoteQueue>,
    reset_version: u64,
}

impl ConsumerLease {
    pub(crate) fn acquire(queue: Arc<NoteQueue>) -> Result<Self, String> {
        queue
            .claimed
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| "This Synth already has an audio consumer".to_owned())?;
        Ok(Self {
            queue,
            reset_version: 0,
        })
    }

    /// One snapshot attempt, at most CAPACITY events, and no retries. A reset
    /// racing this buffer is either applied here or at the next boundary.
    pub(crate) fn dispatch(&mut self, synth: &mut PolySynth) {
        let queue = &self.queue;
        let version = queue.reset_version.load(Ordering::SeqCst);
        if version & 1 != 0 {
            return;
        }
        let boundary = queue.reset_boundary.load(Ordering::SeqCst);
        // Snapshot the initial batch inside the reset-version check: notes
        // published after a concurrent reset must not precede that reset.
        let end = queue.tail.load(Ordering::SeqCst);
        if queue.reset_version.load(Ordering::SeqCst) != version {
            return;
        }
        let mut head = queue.head.load(Ordering::SeqCst);
        if version != self.reset_version {
            synth.all_notes_off();
            head = head.max(boundary);
            self.reset_version = version;
        }
        for position in head..end {
            let event = queue.slots[(position % CAPACITY as u64) as usize].load(Ordering::SeqCst);
            let _ = synth.note_on((event & 127) as u8, (event >> 8) as u8);
        }
        queue.head.store(end, Ordering::SeqCst);
    }
}

impl Drop for ConsumerLease {
    fn drop(&mut self) {
        self.queue.claimed.store(false, Ordering::SeqCst);
    }
}
