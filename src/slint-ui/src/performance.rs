//! Opt-in measurement of the real window, renderer and event loop.
use crate::{MainWindow, Modulation};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

pub struct Performance {
    start: Instant,
    enabled: bool,
    seconds: u64,
    workload: &'static str,
    pub no_audio: bool,
    data: Rc<RefCell<Data>>,
    timer: slint::Timer,
}
#[derive(Default)]
struct Data {
    stages: Vec<(&'static str, f64)>,
    first_frame: Option<f64>,
    rendering: Option<Instant>,
    frames: Vec<f64>,
    lateness: Vec<f64>,
    completed: bool,
}
impl Performance {
    pub fn new() -> Result<Self, String> {
        let start = Instant::now();
        let mut enabled = false;
        let mut seconds = 5;
        let mut workload = "idle";
        let mut no_audio = false;
        for arg in std::env::args().skip(1) {
            match arg.as_str() {
                "--perf" => enabled = true,
                "--perf-no-audio" => no_audio = true,
                "--perf-workload=idle" => workload = "idle",
                "--perf-workload=controls" => workload = "controls",
                "--perf-workload=modulation" => workload = "modulation",
                "--perf-workload=resize" => workload = "resize",
                _ if arg.starts_with("--perf-seconds=") => {
                    seconds = arg[15..]
                        .parse()
                        .map_err(|_| "Invalid performance duration")?;
                    if !(1..=300).contains(&seconds) {
                        return Err("Performance duration must be 1..300 seconds".into());
                    }
                }
                _ => return Err(format!("Unknown argument: {arg}")),
            }
        }
        if !enabled && (no_audio || workload != "idle" || seconds != 5) {
            return Err("Performance options require --perf".into());
        }
        let mut data = Data::default();
        if enabled {
            data.frames.reserve(seconds as usize * 240);
            data.lateness.reserve(seconds as usize * 240);
        }
        Ok(Self {
            start,
            enabled,
            seconds,
            workload,
            no_audio,
            data: Rc::new(RefCell::new(data)),
            timer: slint::Timer::default(),
        })
    }
    pub fn mark(&self, name: &'static str) {
        if self.enabled {
            self.data
                .borrow_mut()
                .stages
                .push((name, self.start.elapsed().as_secs_f64() * 1000.0));
        }
    }
    pub fn attach(&self, window: &MainWindow) -> Result<(), Box<dyn std::error::Error>> {
        if !self.enabled {
            return Ok(());
        }
        let data = self.data.clone();
        let start = self.start;
        window
            .window()
            .set_rendering_notifier(move |state, _| {
                let mut data = data.borrow_mut();
                match state {
                    slint::RenderingState::BeforeRendering => data.rendering = Some(Instant::now()),
                    slint::RenderingState::AfterRendering => {
                        if data.first_frame.is_none() {
                            data.first_frame = Some(start.elapsed().as_secs_f64() * 1000.0);
                        }
                        if let Some(t) = data.rendering.take() {
                            data.frames.push(t.elapsed().as_secs_f64() * 1000.0);
                        }
                    }
                    _ => {}
                }
            })
            .map_err(|e| {
                format!("Performance measurement requires rendering notifications: {e:?}")
            })?;
        let weak = window.as_weak();
        let data = self.data.clone();
        let workload = self.workload;
        let duration = Duration::from_secs(self.seconds);
        let mut first_tick = None;
        let mut previous = Instant::now();
        let mut tick = 0;
        self.timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(16),
            move || {
                let now = Instant::now();
                // Do not confuse first-frame startup with steady-state event-loop stalls.
                if let Some(first) = first_tick {
                    data.borrow_mut()
                        .lateness
                        .push(now.duration_since(previous).as_secs_f64() * 1000.0 - 16.0);
                    if now.duration_since(first) >= duration {
                        data.borrow_mut().completed = true;
                        let _ = slint::quit_event_loop();
                        return;
                    }
                } else {
                    first_tick = Some(now);
                }
                previous = now;
                if let Some(window) = weak.upgrade() {
                    if workload == "controls" {
                        window.invoke_global_edited(6, if tick % 2 == 0 { 800.0 } else { 12000.0 });
                    } else if workload == "modulation" && tick == 0 {
                        for target in 0..plasma_api::TARGET_COUNT as i32 {
                            window.global::<Modulation>().invoke_routed(target, 0, 0.25);
                            window.global::<Modulation>().invoke_routed(target, 1, 0.2);
                        }
                    } else if workload == "resize" && tick % 15 == 0 {
                        let size = if tick % 30 == 0 {
                            (1100.0, 720.0)
                        } else {
                            (1280.0, 820.0)
                        };
                        window
                            .window()
                            .set_size(slint::LogicalSize::new(size.0, size.1));
                    }
                    tick += 1;
                }
            },
        );
        Ok(())
    }
    pub fn report(&self) -> Result<(), Box<dyn std::error::Error>> {
        if !self.enabled {
            return Ok(());
        }
        self.timer.stop();
        let mut data = self.data.borrow_mut();
        let first = data.first_frame.ok_or("No rendered frame was observed")?;
        if !data.completed {
            return Err("Performance run was closed before completion".into());
        }
        if !self.no_audio
            && (!data
                .stages
                .iter()
                .any(|(name, _)| *name == "audio_ready_ms")
                || data
                    .stages
                    .iter()
                    .any(|(name, _)| *name == "audio_failed_ms"))
        {
            return Err("Audio did not become ready or failed during measurement; use --perf-no-audio for explicit isolation".into());
        }
        println!("{{\"metric\":\"first_frame_ms\",\"value\":{first:.6}}}");
        println!(
            "{{\"metric\":\"profile\",\"value\":\"{}\"}}",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        );
        for (name, value) in &data.stages {
            println!("{{\"metric\":\"{name}\",\"value\":{value:.6}}}");
        }
        let data = &mut *data;
        for (name, values) in [
            ("render_ms", &mut data.frames),
            ("event_loop_lateness_ms", &mut data.lateness),
        ] {
            values.sort_by(f64::total_cmp);
            if values.is_empty() {
                return Err(format!("No samples for {name}").into());
            }
            let percentile = |p: f64| {
                values[((values.len() as f64 * p).ceil() as usize)
                    .saturating_sub(1)
                    .min(values.len() - 1)]
            };
            println!(
                "{{\"metric\":\"{name}\",\"count\":{},\"p50\":{:.6},\"p95\":{:.6},\"p99\":{:.6},\"max\":{:.6}}}",
                values.len(),
                percentile(0.5),
                percentile(0.95),
                percentile(0.99),
                values[values.len() - 1]
            );
        }
        Ok(())
    }
}
