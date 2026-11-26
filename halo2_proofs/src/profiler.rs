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

 
/// Record key-specific metrics separately from admission-stage status.
pub(crate) fn key_metric(component: &str, operation: &str, bytes: Option<u64>, seconds: f64) {
    if !enabled() { return; }
    let Some(p) = path("ZKINFER_HALO2_KEY_METRICS_FILE", "halo2_key_metrics.csv") else { return };
    let exists = p.exists();
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(p) {
        if !exists { let _ = writeln!(f, "component,operation,bytes,duration_s"); }
        let bytes = bytes.map(|v| v.to_string()).unwrap_or_default();
        let _ = writeln!(f, "{component},{operation},{bytes},{seconds:.9}");
    }
}

/// Measure a serialization component without allocating a second copy.
pub(crate) fn key_write<W: Write>(
    writer: &mut W, component: &str,
    write: impl FnOnce(&mut W) -> std::io::Result<()>,
    bytes: usize,
) -> std::io::Result<()> {
    let start = Instant::now();
    write(writer)?;
    key_metric(component, "serialize", Some(bytes as u64), start.elapsed().as_secs_f64());
    Ok(())
}

/// Time a key construction or reconstruction operation.
pub(crate) struct KeyTimer { component: &'static str, start: Instant }
impl KeyTimer {
    pub(crate) fn start(component: &'static str) -> Self {
        Self { component, start: Instant::now() }
    }
}
impl Drop for KeyTimer {
    fn drop(&mut self) {
        key_metric(self.component, "keygen", None, self.start.elapsed().as_secs_f64());
    }
}

/// Count logical serialized bytes consumed by a successful component read.
/// The underlying buffered reader remains responsible for physical I/O.
pub(crate) fn key_read<R: std::io::Read, T>(
    reader: &mut R, component: &str,
    read: impl FnOnce(&mut CountingReader<'_, R>) -> std::io::Result<T>,
) -> std::io::Result<T> {
    let start = Instant::now();
    let mut counted = CountingReader { inner: reader, bytes: 0 };
    let value = read(&mut counted)?;
    key_metric(component, "deserialize", Some(counted.bytes), start.elapsed().as_secs_f64());
    Ok(value)
}

pub(crate) struct CountingReader<'a, R> {
    inner: &'a mut R,
    bytes: u64,
}
impl<R: std::io::Read> std::io::Read for CountingReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes += n as u64;
        Ok(n)
    }
    fn read_exact(&mut self, buf: &mut [u8]) -> std::io::Result<()> {
        self.inner.read_exact(buf)?;
        self.bytes += buf.len() as u64;
        Ok(())
    }
}
