//! Test support: a fake Pitwall on a random pipe, a temp workspace with a temp key.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use stintmate_ipc::blocking::Endpoint;
use stintmate_ipc::codec::Value;
use stintmate_ipc::error::RemoteError;
use stintmate_ipc::handshake::PeerInfo;
use stintmate_ipc::keyfile::{Key, KeySource, format_key};
use stintmate_ipc::{MethodOpts, naming};

/// The key of the tests (never the driver's).
pub const KEY: Key = [7u8; 32];

/// One call the fake received.
#[derive(Clone, Debug)]
pub struct Call {
    pub method: String,
    pub ik: Option<String>,
    pub params: Value,
}

/// What the fake does besides answering `ok`.
#[derive(Default)]
pub struct Rules {
    /// Answer `bad_request` to the next `participant_laps` write with more than one row.
    pub refuse_next_batch: bool,
}

pub struct FakePitwall {
    endpoint: Endpoint,
    pub pipe: String,
    pub calls: Arc<Mutex<Vec<Call>>>,
    pub peers: Arc<Mutex<Vec<PeerInfo>>>,
    pub rules: Arc<Mutex<Rules>>,
}

static N: AtomicU64 = AtomicU64::new(0);

pub fn unique_user() -> String {
    format!(
        "t{}x{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

/// A temp workspace holding `ipc\key` for [`KEY`].
pub fn workspace(dir: &Path, key: &Key) -> PathBuf {
    let ipc = dir.join("ipc");
    std::fs::create_dir_all(&ipc).unwrap();
    std::fs::write(ipc.join("key"), format_key(key)).unwrap();
    dir.to_path_buf()
}

pub fn pitwall_pipe(user: &str) -> String {
    naming::pipe_path_for("pitwall", None, user).unwrap()
}

impl FakePitwall {
    pub fn start(pipe: &str) -> Self {
        Self::start_with_key(pipe, KEY)
    }

    pub fn start_with_key(pipe: &str, key: Key) -> Self {
        let endpoint = Endpoint::builder(PeerInfo::new("pitwall"), KeySource::Bytes(key))
            .pipe(pipe)
            .build()
            .unwrap();
        let calls: Arc<Mutex<Vec<Call>>> = Arc::default();
        let peers: Arc<Mutex<Vec<PeerInfo>>> = Arc::default();
        let rules: Arc<Mutex<Rules>> = Arc::default();
        let lap_n = Arc::new(AtomicU64::new(0));
        let methods = [
            "pitwall.write.catalog",
            "pitwall.write.session_start",
            "pitwall.write.lap",
            "pitwall.write.setup",
            "pitwall.write.pit_exit",
            "pitwall.write.participant_laps",
            "pitwall.write.session_end",
            "pitwall.catalog.declare",
            "pitwall.guidance.declare",
            "pitwall.actions.declare",
            "pitwall.announce",
        ];
        for m in methods {
            let (c, p, r, n) = (calls.clone(), peers.clone(), rules.clone(), lap_n.clone());
            let name = m.to_owned();
            endpoint
                .method(
                    m,
                    MethodOpts::default().ordered(true).idempotent(true),
                    move |ctx, params| {
                        if name == "pitwall.write.participant_laps" {
                            let rows = stintmate_ipc::codec::get(&params, "laps")
                                .and_then(Value::as_array)
                                .map_or(0, Vec::len);
                            let mut rules = r.lock().unwrap();
                            if rules.refuse_next_batch && rows > 1 {
                                rules.refuse_next_batch = false;
                                return Err(RemoteError::new("bad_request", "an odd row"));
                            }
                        }
                        p.lock().unwrap().push(ctx.peer().info().clone());
                        c.lock().unwrap().push(Call {
                            method: name.clone(),
                            ik: ctx.idempotency_key().map(str::to_owned),
                            params,
                        });
                        Ok(match name.as_str() {
                            "pitwall.write.session_start" => {
                                stintmate_ipc::codec::map([("session_id", Value::from(1u32))])
                            }
                            "pitwall.write.lap" => stintmate_ipc::codec::map([
                                ("lap", Value::from(n.fetch_add(1, Ordering::Relaxed) + 1)),
                                ("stint", Value::from(1u32)),
                            ]),
                            "pitwall.announce" => stintmate_ipc::codec::map([
                                ("message_id", Value::from("m1")),
                                ("at", Value::from("T")),
                            ]),
                            _ => stintmate_ipc::codec::map::<[(&str, Value); 0], &str>([]),
                        })
                    },
                )
                .unwrap();
        }
        endpoint.start().unwrap();
        Self {
            endpoint,
            pipe: pipe.to_owned(),
            calls,
            peers,
            rules,
        }
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    pub fn count(&self, method: &str) -> usize {
        self.calls().iter().filter(|c| c.method == method).count()
    }

    pub fn wait_for(&self, what: impl Fn(&[Call]) -> bool, secs: u64) -> bool {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            if what(&self.calls.lock().unwrap()) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// The recorder's connection, once it is up.
    pub fn peer(&self) -> Option<stintmate_ipc::blocking::Peer> {
        self.endpoint.peers().into_iter().next()
    }

    pub fn stop(&self) {
        self.endpoint.stop();
    }
}

/// A rmpv value as JSON (`bin` as an array of bytes).
pub fn to_json(v: &Value) -> serde_json::Value {
    use serde_json::{Map, json};
    match v {
        Value::Nil => serde_json::Value::Null,
        Value::Boolean(b) => json!(b),
        Value::Integer(i) => i.as_i64().map_or_else(|| json!(i.as_u64()), |x| json!(x)),
        Value::F32(f) => json!(f64::from(*f)),
        Value::F64(f) => json!(f),
        Value::String(s) => json!(s.as_str()),
        Value::Binary(b) => json!(b),
        Value::Array(a) => serde_json::Value::Array(a.iter().map(to_json).collect()),
        Value::Map(m) => serde_json::Value::Object(
            m.iter()
                .map(|(k, v)| (k.as_str().unwrap_or_default().to_owned(), to_json(v)))
                .collect::<Map<_, _>>(),
        ),
        Value::Ext(..) => serde_json::Value::Null,
    }
}
