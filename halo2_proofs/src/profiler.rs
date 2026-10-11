use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use sysinfo::{CpuExt, System, SystemExt};

use crate::plonk::ProverLoggingInfo;

const OUTER_PROVER_STAGE: &str = "PROVING";

fn now_unix_micros() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros())
        .unwrap_or(0)
}

fn stage_file() -> Option<PathBuf> {
    if let Ok(path) = env::var("ZKINFER_STAGE_FILE") {
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    env::var("EZKL_LOG_DIR")
        .ok()
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(path).join("status.txt"))
}

fn transition_file() -> Option<PathBuf> {
    if let Ok(path) = env::var("ZKINFER_HALO2_STAGE_TRANSITIONS_FILE") {
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    env::var("EZKL_LOG_DIR")
        .ok()
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(path).join("halo2_stage_transitions.csv"))
}

/// Export the currently executing Halo2 prover subphase to the host runtime.
fn write_stage(stage: &str) {
    if let Some(path) = stage_file() {
        let _ = std::fs::write(path, stage);
    }
}

/// Append exact start/end events for Halo2 subphases.
///
/// Start rows leave duration columns empty. End rows contain the elapsed time
/// measured by the same monotonic Instant that brackets the profiled region.
/// Keeping both event rows makes incomplete/crashed regions visible while also
/// making completed phase durations directly usable by analysis code.
fn append_transition(event: &str, phase_key: &str, duration_s: Option<f64>) {
    let Some(path) = transition_file() else {
        return;
    };

    let file_exists = path.exists();
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        if !file_exists {
            let _ = writeln!(
                file,
                "timestamp_unix_us,event,stage,duration_us,duration_s"
            );
        }

        match duration_s {
            Some(seconds) => {
                let duration_us = (seconds * 1_000_000.0).round() as u128;
                let _ = writeln!(
                    file,
                    "{},{},PROVING::{},{},{:.9}",
                    now_unix_micros(),
                    event,
                    phase_key,
                    duration_us,
                    seconds
                );
            }
            None => {
                let _ = writeln!(
                    file,
                    "{},{},PROVING::{},,",
                    now_unix_micros(),
                    event,
                    phase_key
                );
            }
        }
        let _ = file.flush();
    }
}

fn begin_phase(phase_key: &str) -> Instant {
    let started = Instant::now();
    let stage = format!("PROVING::{phase_key}");
    write_stage(&stage);
    append_transition("start", phase_key, None);
    started
}

fn end_phase(phase_key: &str, started: Instant) {
    append_transition("end", phase_key, Some(started.elapsed().as_secs_f64()));
    write_stage(OUTER_PROVER_STAGE);
}

#[derive(Debug)]
pub struct Profiler {
    system: System,
}

impl Profiler {
    pub fn new() -> Self {
        let mut system = System::new_all();
        system.refresh_cpu();
        Profiler { system }
    }

    pub fn log(&self, label: &str, duration: f64) {
        println!("📊 {:<35} | ⏱ {:>6.3}s", label, duration);
    }

    pub fn measure<F, T>(
        &mut self,
        label: &str,
        stat_collector: &mut ProverLoggingInfo,
        phase_key: &str,
        f: F,
    ) -> T
    where
        F: FnOnce() -> T,
    {
        let phase_started = begin_phase(phase_key);
        let start = Instant::now();
        self.system.refresh_cpu();

        let result = f();

        self.system.refresh_cpu();
        let elapsed = start.elapsed().as_secs_f64();
        let cpu = self.system.global_cpu_info().cpu_usage();

        println!("📊 {:<35} | ⏱ {:>6.3}s | 🧠 {:>5.2}% CPU", label, elapsed, cpu);

        stat_collector.insert(phase_key, elapsed, cpu);
        end_phase(phase_key, phase_started);
        result
    }

    pub fn measure_result<T, E>(
        &mut self,
        label: &str,
        stat_collector: &mut ProverLoggingInfo,
        field: &str,
        f: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        let phase_started = begin_phase(field);
        let start = Instant::now();
        self.system.refresh_cpu();

        let result = f();

        self.system.refresh_cpu();
        let duration = start.elapsed().as_secs_f64();
        let cpu = self.system.global_cpu_info().cpu_usage();

        println!("📊 {:<35} | ⏱ {:>6.3}s | 🧠 {:>5.2}% CPU", label, duration, cpu);

        stat_collector.insert(field, duration, cpu);
        end_phase(field, phase_started);
        result
    }

    pub fn measure_result_value<T, F>(
        &self,
        label: &str,
        stat_collector: &mut ProverLoggingInfo,
        stat_field: &str,
        f: F,
    ) -> T
    where
        F: FnOnce() -> T,
    {
        let phase_started = begin_phase(stat_field);
        let start_time = Instant::now();
        let result = f();
        let duration = start_time.elapsed().as_secs_f64();

        self.log(label, duration);
        stat_collector.insert(stat_field, duration, 0.0);
        end_phase(stat_field, phase_started);

        result
    }
}
