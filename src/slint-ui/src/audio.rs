//! The device stream is created, monitored and dropped on its owning thread.
//! CPAL streams need not be Send; only status strings cross to the UI.
use plasma_api::{AudioOutput, Synth};
use std::{
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
    time::Duration,
};

pub struct AudioWorker {
    pub events: Receiver<Result<String, String>>,
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}
impl AudioWorker {
    pub fn start(synth: Synth) -> Result<Self, String> {
        let (events_tx, events) = mpsc::sync_channel(2);
        let (stop, stopped) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("plasma-audio-device".into())
            .spawn(move || {
                let output = match AudioOutput::start(synth) {
                    Ok(output) => output,
                    Err(error) => {
                        let _ = events_tx.send(Err(error));
                        return;
                    }
                };
                if events_tx.send(Ok(output.description().to_owned())).is_err() {
                    return;
                }
                loop {
                    match stopped.recv_timeout(Duration::from_millis(250)) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if let Some(error) = output.error() {
                                let _ = events_tx.send(Err(error));
                                break;
                            }
                        }
                    }
                }
                // Stream destruction remains on the thread that created it.
            })
            .map_err(|error| format!("Cannot start audio device worker: {error}"))?;
        Ok(Self {
            events,
            stop: Some(stop),
            thread: Some(thread),
        })
    }
}
impl Drop for AudioWorker {
    fn drop(&mut self) {
        // Disconnect wakes the worker without waiting for its polling interval.
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                eprintln!("Audio device worker panicked during shutdown");
            }
        }
    }
}
