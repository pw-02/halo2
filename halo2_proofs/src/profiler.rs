use std::env;
use std::fs::OpenOptions;
use std::io::Write;
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

/// Export the currently executing Halo2 prover subphase to the host runtime.
///
/// zkInfer sets ZKINFER_STAGE_FILE to the same status file sampled by its
/// resource logger. Halo2 itself remains independent of zkInfer: if the env var
/// is absent, this is a no-op. While a measured prover region is running the
/// status becomes `PROVING::<phase_key>` and is restored to `PROVING` when the
/// region ends.
fn write_stage(stage: &str) {
    if let Ok(path) = env::var("ZKINFER_STAGE_FILE") {
        if !path.is_empty() {
            let _ = std::fs::write(path, stage);
        }
    }
}

/// Append an exact transition record in addition to the sampled resource trace.
/// The CSV is intentionally simple so it can be aligned with resource_usage.csv
/// without changing proof semantics or introducing a zkInfer dependency.
fn append_transition(event: &str, phase_key: &str) {
    let path = match env::var("ZKINFER_HALO2_STAGE_TRANSITIONS_FILE") {
        Ok(path) if !path.is_empty() => path,
        _ => return,
    };

    let file_exists = std::path::Path::new(&path).exists();
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        if !file_exists {
            let _ = writeln!(file, "timestamp_unix_us,event,stage");
        }
        let _ = writeln!(
            file,
            "{},{},PROVING::{}",
            now_unix_micros(),
            event,
            phase_key
        );
        let _ = file.flush();
    }
}

fn begin_phase(phase_key: &str) {
    let stage = format!("PROVING::{phase_key}");
    write_stage(&stage);
    append_transition("start", phase_key);
}

fn end_phase(phase_key: &str) {
    append_transition("end", phase_key);
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
        begin_phase(phase_key);
        let start = Instant::now();
        self.system.refresh_cpu();

        let result = f();

        self.system.refresh_cpu();
        let elapsed = start.elapsed().as_secs_f64();
        let cpu = self.system.global_cpu_info().cpu_usage();

        println!("📊 {:<35} | ⏱ {:>6.3}s | 🧠 {:>5.2}% CPU", label, elapsed, cpu);

        stat_collector.insert(phase_key, elapsed, cpu);
        end_phase(phase_key);
        result
    }

    pub fn measure_result<T, E>(
        &mut self,
        label: &str,
        stat_collector: &mut ProverLoggingInfo,
        field: &str,
        f: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        begin_phase(field);
        let start = Instant::now();
        self.system.refresh_cpu();

        let result = f();

        self.system.refresh_cpu();
        let duration = start.elapsed().as_secs_f64();
        let cpu = self.system.global_cpu_info().cpu_usage();

        println!("📊 {:<35} | ⏱ {:>6.3}s | 🧠 {:>5.2}% CPU", label, duration, cpu);

        stat_collector.insert(field, duration, cpu);
        end_phase(field);
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
        begin_phase(stat_field);
        let start_time = Instant::now();
        let result = f();
        let duration = start_time.elapsed().as_secs_f64();

        self.log(label, duration);
        stat_collector.insert(stat_field, duration, 0.0);
        end_phase(stat_field);

        result
    }
}
