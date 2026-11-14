//! The outbox and the writes an older Pitwall may not take (simulator data): a write of an
//! optional method that the linked Pitwall does not declare, or answers with `unknown_method`,
//! is deleted without being refused; every other `unknown_method` is still a refusal.
//!
//! The link is a test double that declares a set of methods and answers as told. The log lines
//! are read from a process-wide subscriber; each test names its Pitwall version differently so
//! it reads only its own lines.

use ams2_recorder::link::{CallError, Transport};
use ams2_recorder::outbox::{OnRejected, Outbox};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use stintmate_ipc::codec::Value;

const KINDS: &str = "pitwall.write.sim_data_kinds";
const SIM_DATA: &str = "pitwall.write.sim_data";
const LAP: &str = "pitwall.write.lap";

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Log {
    type Writer = Log;
    fn make_writer(&'a self) -> Log {
        self.clone()
    }
}

fn log() -> &'static Log {
    static LOG: OnceLock<Log> = OnceLock::new();
    LOG.get_or_init(|| {
        let log = Log::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(log.clone())
            .with_ansi(false)
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
        log
    })
}

fn lines_with(text: &str) -> usize {
    String::from_utf8_lossy(&log().0.lock().unwrap())
        .lines()
        .filter(|l| l.contains(text))
        .count()
}

struct Fake {
    version: &'static str,
    connected: AtomicBool,
    /// The methods the peer declares; `None`: it declares none (unknown).
    declared: Option<HashSet<&'static str>>,
    /// Methods it answers with an error code.
    errors: HashMap<&'static str, &'static str>,
    calls: Mutex<Vec<String>>,
}

impl Fake {
    fn new(version: &'static str, declared: Option<&[&'static str]>) -> Arc<Self> {
        Arc::new(Self {
            version,
            connected: AtomicBool::new(true),
            declared: declared.map(|d| d.iter().copied().collect()),
            errors: HashMap::new(),
            calls: Mutex::default(),
        })
    }

    fn with_error(version: &'static str, method: &'static str, code: &'static str) -> Arc<Self> {
        Arc::new(Self {
            version,
            connected: AtomicBool::new(true),
            declared: None,
            errors: HashMap::from([(method, code)]),
            calls: Mutex::default(),
        })
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl Transport for Fake {
    fn connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }
    fn wait_connected(&self, timeout: Duration) -> bool {
        std::thread::sleep(timeout.min(Duration::from_millis(10)));
        self.connected()
    }
    fn call(
        &self,
        method: &str,
        _: Value,
        _: Option<&str>,
        _: Duration,
    ) -> Result<Value, CallError> {
        self.calls.lock().unwrap().push(method.to_owned());
        match self.errors.get(method) {
            Some(code) => Err(CallError::Remote {
                code: (*code).to_owned(),
                message: "test".to_owned(),
                retry_ms: None,
            }),
            None => Ok(Value::Nil),
        }
    }
    fn call_async(
        &self,
        _: &str,
        _: Value,
        _: Duration,
        _: Box<dyn FnOnce(Result<Value, CallError>) + Send>,
    ) -> Result<(), CallError> {
        Err(CallError::NotConnected)
    }
    fn declares(&self, method: &str) -> Option<bool> {
        self.declared.as_ref().map(|d| d.contains(method))
    }
    fn peer_version(&self) -> Option<String> {
        Some(self.version.to_owned())
    }
}

struct Rig {
    outbox: Outbox,
    dir: tempfile::TempDir,
    rejected_calls: Arc<Mutex<Vec<(String, String)>>>,
}

fn rig(link: Arc<Fake>) -> Rig {
    let _ = log();
    let dir = tempfile::tempdir().unwrap();
    let rejected_calls = Arc::new(Mutex::new(Vec::new()));
    let r = rejected_calls.clone();
    let on_rejected: OnRejected = Box::new(move |m, _, code| {
        r.lock().unwrap().push((m.to_owned(), code.to_owned()));
    });
    let (outbox, _sender) = Outbox::start(
        &dir.path().join("outbox"),
        link,
        None,
        Some(on_rejected),
        None,
    )
    .unwrap();
    Rig {
        outbox,
        dir,
        rejected_calls,
    }
}

impl Rig {
    fn put(&self, method: &str, n: u32) {
        self.outbox
            .put(method, &format!("ams2/test/{n}"), Value::Nil)
            .unwrap();
    }

    fn rejected_files(&self) -> usize {
        std::fs::read_dir(self.dir.path().join("outbox").join("rejected"))
            .map(|d| d.count())
            .unwrap_or(0)
    }
}

#[test]
fn a_method_the_peer_does_not_declare_is_skipped_not_refused() {
    let link = Fake::new("test-undeclared", Some(&[LAP]));
    let rig = rig(link.clone());
    rig.put(KINDS, 1);
    rig.put(SIM_DATA, 2);
    rig.put(SIM_DATA, 3);
    rig.put(LAP, 4);
    assert!(rig.outbox.flush(Duration::from_secs(5)));
    assert_eq!(link.calls(), [LAP], "only the declared method is sent");
    let s = rig.outbox.summary();
    assert_eq!((s.pending, s.rejected), (0, 0));
    assert_eq!(rig.rejected_files(), 0);
    assert!(rig.rejected_calls.lock().unwrap().is_empty());
    // Once per method per connection.
    assert_eq!(
        lines_with("Pitwall test-undeclared does not take pitwall.write.sim_data;"),
        1
    );
    assert_eq!(
        lines_with("Pitwall test-undeclared does not take pitwall.write.sim_data_kinds;"),
        1
    );
    rig.outbox.close(Duration::ZERO);
}

#[test]
fn a_reconnection_logs_the_skip_again() {
    let link = Fake::new("test-reconnect", Some(&[LAP]));
    let rig = rig(link.clone());
    rig.put(SIM_DATA, 1);
    assert!(rig.outbox.flush(Duration::from_secs(5)));
    assert_eq!(lines_with("Pitwall test-reconnect does not take"), 1);
    // The link drops with a write waiting, then comes back (maybe another Pitwall).
    link.connected.store(false, Ordering::Relaxed);
    rig.put(SIM_DATA, 2);
    std::thread::sleep(Duration::from_millis(300));
    link.connected.store(true, Ordering::Relaxed);
    assert!(rig.outbox.flush(Duration::from_secs(5)));
    assert_eq!(lines_with("Pitwall test-reconnect does not take"), 2);
    assert!(link.calls().is_empty());
    rig.outbox.close(Duration::ZERO);
}

#[test]
fn an_unknown_method_answer_to_simulator_data_is_skipped_and_the_next_write_goes() {
    let link = Fake::with_error("test-unknown", SIM_DATA, "unknown_method");
    let rig = rig(link.clone());
    rig.put(SIM_DATA, 1);
    rig.put(LAP, 2);
    assert!(rig.outbox.flush(Duration::from_secs(5)));
    assert_eq!(link.calls(), [SIM_DATA, LAP]);
    let s = rig.outbox.summary();
    assert_eq!((s.pending, s.rejected), (0, 0));
    assert_eq!(rig.rejected_files(), 0);
    assert!(rig.rejected_calls.lock().unwrap().is_empty());
    assert_eq!(
        lines_with("Pitwall test-unknown does not take pitwall.write.sim_data;"),
        1
    );
    rig.outbox.close(Duration::ZERO);
}

#[test]
fn an_unknown_method_answer_to_a_lap_is_still_a_refusal() {
    let link = Fake::with_error("test-lap", LAP, "unknown_method");
    let rig = rig(link.clone());
    rig.put(LAP, 1);
    assert!(rig.outbox.flush(Duration::from_secs(5)));
    let s = rig.outbox.summary();
    assert_eq!((s.pending, s.rejected), (0, 1));
    assert_eq!(rig.rejected_files(), 1);
    assert_eq!(
        *rig.rejected_calls.lock().unwrap(),
        [(LAP.to_owned(), "unknown_method".to_owned())]
    );
    assert_eq!(lines_with("Pitwall test-lap does not take"), 0);
    rig.outbox.close(Duration::ZERO);
}
