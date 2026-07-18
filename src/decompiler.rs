//! Background decompiler: a dedicated rizin instance that runs Ghidra's `pdgj`
//! off the UI thread. `pdgj` on a large function can take seconds, so it must
//! never block input. Requests are coalesced (only the newest function matters)
//! and rename/comment edits are forwarded so decompiled names stay in sync with
//! the main instance.

use crate::backend::DecompResult;
use crate::pipe::RzPipe;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::{self, JoinHandle};

enum DecompRequest {
    Decompile(u64),
    /// forward an edit command (rename/comment) to keep this instance in sync
    Exec(String),
    Shutdown,
}

pub enum DecompEvent {
    /// analysis finished — the decompiler is now usable
    Ready,
    Done { addr: u64, result: Box<DecompResult> },
    Failed { addr: u64, error: String },
}

pub struct Decompiler {
    tx: Sender<DecompRequest>,
    rx: Receiver<DecompEvent>,
    handle: Option<JoinHandle<()>>,
    pub ready: bool,
}

impl Decompiler {
    pub fn spawn(file: String, project: Option<String>) -> Self {
        let (req_tx, req_rx) = channel();
        let (msg_tx, msg_rx) = channel();
        let handle =
            thread::spawn(move || worker(&file, project.as_deref(), &req_rx, &msg_tx));
        Self {
            tx: req_tx,
            rx: msg_rx,
            handle: Some(handle),
            ready: false,
        }
    }

    pub fn request(&self, addr: u64) {
        let _ = self.tx.send(DecompRequest::Decompile(addr));
    }

    /// Apply an edit command on the decompiler instance too (keeps names synced).
    pub fn forward(&self, cmd: String) {
        let _ = self.tx.send(DecompRequest::Exec(cmd));
    }

    /// Drain all pending messages without blocking.
    pub fn poll(&mut self) -> Vec<DecompEvent> {
        let mut out = Vec::new();
        while let Ok(m) = self.rx.try_recv() {
            if matches!(m, DecompEvent::Ready) {
                self.ready = true;
            }
            out.push(m);
        }
        out
    }
}

impl Drop for Decompiler {
    fn drop(&mut self) {
        let _ = self.tx.send(DecompRequest::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn worker(file: &str, project: Option<&str>, rx: &Receiver<DecompRequest>, tx: &Sender<DecompEvent>) {
    let Ok(mut pipe) = RzPipe::open(file, false, project) else {
        return;
    };
    // Decompilation quality depends on analysis; run it once up front.
    let _ = pipe.cmd("aaa");
    if tx.send(DecompEvent::Ready).is_err() {
        return;
    }

    loop {
        // Block until there's work, then drain the whole queue so we only run
        // the *latest* decompile request (older ones are stale). Edit commands
        // are applied in order so the final decompile reflects them.
        let mut target: Option<u64> = None;
        let Ok(first) = rx.recv() else {
            break;
        };
        let mut queue = vec![first];
        while let Ok(r) = rx.try_recv() {
            queue.push(r);
        }
        let mut shutdown = false;
        for r in queue {
            match r {
                DecompRequest::Decompile(addr) => target = Some(addr),
                DecompRequest::Exec(cmd) => {
                    let _ = pipe.cmd(&cmd);
                }
                DecompRequest::Shutdown => {
                    shutdown = true;
                    break;
                }
            }
        }
        if let Some(addr) = target {
            let msg = match pipe.cmdj(&format!("pdgj @ {addr:#x}")) {
                Ok(v) => match serde_json::from_value::<DecompResult>(v) {
                    Ok(res) => DecompEvent::Done {
                        addr,
                        result: Box::new(res),
                    },
                    Err(e) => DecompEvent::Failed {
                        addr,
                        error: e.to_string(),
                    },
                },
                Err(e) => DecompEvent::Failed {
                    addr,
                    error: format!("{e:#}"),
                },
            };
            if tx.send(msg).is_err() {
                break;
            }
        }
        if shutdown {
            break;
        }
    }
}

/// Small LRU cache of decompiled functions keyed by entry address.
pub struct DecompCache {
    cap: usize,
    items: Vec<(u64, DecompResult)>,
}

impl DecompCache {
    pub const fn new(cap: usize) -> Self {
        Self {
            cap,
            items: Vec::new(),
        }
    }

    /// Return a clone of the cached result, marking it most-recently-used.
    pub fn get(&mut self, addr: u64) -> Option<DecompResult> {
        let pos = self.items.iter().position(|(a, _)| *a == addr)?;
        let entry = self.items.remove(pos);
        let result = entry.1.clone();
        self.items.push(entry);
        Some(result)
    }

    pub fn put(&mut self, addr: u64, result: DecompResult) {
        self.items.retain(|(a, _)| *a != addr);
        self.items.push((addr, result));
        while self.items.len() > self.cap {
            self.items.remove(0);
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }
}
