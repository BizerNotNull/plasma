//! Allocation is a hard realtime contract; wall-clock budgets belong in the
//! performance example rather than machine-dependent unit-test assertions.
use plasma_kernel::{TARGET_COUNT, Voice, VoiceParams, Waveform};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    hint::black_box,
};

thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}
struct CountingAllocator;
fn count() {
    let _ = TRACK.try_with(|track| {
        if track.get() {
            let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        }
    });
}
// SAFETY: all allocations are forwarded unchanged to the system allocator.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct Tracking;
impl Drop for Tracking {
    fn drop(&mut self) {
        TRACK.set(false);
    }
}

#[test]
fn realtime_render_and_control_transitions_do_not_allocate() {
    let mut voice = Voice::new(48000.0, 42).unwrap();
    let mut params = VoiceParams::default();
    params.routes = [[0.025; TARGET_COUNT], [-0.02; TARGET_COUNT]];
    params.globals[3] = 0.001;
    for osc in &mut params.oscillators {
        osc.unison = 4;
        osc.level = 0.25;
    }
    let mut frames = [[0.0; 2]; 256];
    ALLOCATIONS.set(0);
    TRACK.set(true);
    let tracking = Tracking;
    voice.render(black_box(&mut frames)); // idle, including free LFO
    for wave in [
        Waveform::Sine,
        Waveform::Triangle,
        Waveform::Saw,
        Waveform::Pulse,
    ] {
        for osc in &mut params.oscillators {
            osc.waveform = wave;
        }
        voice.set_params(black_box(params)).unwrap();
        voice.note_on(220.0).unwrap();
        for _ in 0..32 {
            voice.render(black_box(&mut frames));
        }
        voice.note_off();
        voice.render(black_box(&mut frames));
        voice.note_on(440.0).unwrap();
        voice.set_params(black_box(params)).unwrap();
        voice.render(black_box(&mut frames));
        black_box(voice.telemetry());
    }
    drop(tracking);
    assert_eq!(ALLOCATIONS.get(), 0, "audio-thread operations allocated");
}
