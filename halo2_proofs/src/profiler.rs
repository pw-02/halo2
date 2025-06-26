use std::time::Instant;
use sysinfo::{System, SystemExt, CpuExt};

use crate::plonk::ProverLoggingInfo;

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
        let start = Instant::now();
        self.system.refresh_cpu();

        let result = f();

        self.system.refresh_cpu();
        let elapsed = start.elapsed().as_secs_f64();
        let cpu = self.system.global_cpu_info().cpu_usage();

        println!("📊 {:<35} | ⏱ {:>6.3}s | 🧠 {:>5.2}% CPU", label, elapsed, cpu);

        stat_collector.insert(phase_key, elapsed, cpu);
        result
    }

    pub fn measure_result<T, E>(
        &mut self,
        label: &str,
        stat_collector: &mut ProverLoggingInfo,
        field: &str,
        f: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        let start = Instant::now();
        self.system.refresh_cpu();

        let result = f();

        self.system.refresh_cpu();
        let duration = start.elapsed().as_secs_f64();
        let cpu = self.system.global_cpu_info().cpu_usage();

        println!("📊 {:<35} | ⏱ {:>6.3}s | 🧠 {:>5.2}% CPU", label, duration, cpu);

        stat_collector.insert(field, duration, cpu);
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
    let start_time = Instant::now();
    let result = f();
    let duration = start_time.elapsed().as_secs_f64();

    self.log(label, duration);

    // 👇 Replace this:
    // stat_collector.set(stat_field, duration);
    // 👇 With this:
    stat_collector.insert(stat_field, duration, 0.0);

    result
}

}
