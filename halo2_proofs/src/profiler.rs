//! Optional monotonic prover-phase timings for zkInfer.
use std::{env, fs::OpenOptions, io::Write, path::PathBuf, time::{Instant, SystemTime, UNIX_EPOCH}};
fn path(var: &str, filename: &str) -> Option<PathBuf> {
    env::var(var).ok().filter(|v| !v.is_empty()).map(PathBuf::from)
        .or_else(|| env::var("EZKL_LOG_DIR").ok().filter(|v| !v.is_empty()).map(|v| PathBuf::from(v).join(filename)))
}
fn enabled() -> bool { env::var("ZKINFER_PROFILING_MODE").as_deref() != Ok("basic") }
fn stage(value: &str) {
    if let Some(p) = path("ZKINFER_STAGE_FILE", "status.txt") { let _ = std::fs::write(p, value); }
}
fn event(name: &str, kind: &str, elapsed: Option<f64>) {
    let Some(p) = path("ZKINFER_HALO2_STAGE_TRANSITIONS_FILE", "halo2_stage_transitions.csv") else { return };
    let exists = p.exists();
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(p) {
        if !exists { let _ = writeln!(f, "timestamp_unix_us,event,stage,duration_us,duration_s"); }
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|v| v.as_micros()).unwrap_or(0);
        match elapsed {
            Some(s) => { let _ = writeln!(f, "{now},{kind},PROVING::{name},{},{s:.9}", (s*1e6).round() as u128); },
            None => { let _ = writeln!(f, "{now},{kind},PROVING::{name},,"); }
        }
    }
}
/// Record actual parallel pool size.
pub fn record_threads() {
    if let Ok(dir) = env::var("EZKL_LOG_DIR") {
        let _ = std::fs::write(PathBuf::from(dir).join("rayon_pool_size.txt"), crate::multicore::current_num_threads().to_string());
    }
}
/// A phase guard that records timing without changing the proving computation.
pub struct Phase { name: &'static str, start: Instant, active: bool }
impl Phase {
    /// Begin a phase.
    pub fn start(name: &'static str) -> Self {
        let active = enabled();
        if active { stage(&format!("PROVING::{name}")); event(name, "start", None); }
        Self { name, start: Instant::now(), active }
    }
}
impl Drop for Phase {
    fn drop(&mut self) {
        if self.active { event(self.name, "end", Some(self.start.elapsed().as_secs_f64())); stage("PROVING"); }
    }
}
