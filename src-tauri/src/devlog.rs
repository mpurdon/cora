use std::collections::VecDeque;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use ts_rs::TS;

pub const DEV_LOG_EVENT: &str = "dev:log";
const CAPACITY: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub at: String,
    pub level: LogLevel,
    /// Subsystem: poller | github | bedrock | analysis | app
    pub source: String,
    pub message: String,
}

/// In-memory ring buffer of recent internals; drives the Developer panel.
pub struct DevLog(Mutex<VecDeque<LogEntry>>);

impl DevLog {
    pub fn new() -> Self {
        Self(Mutex::new(VecDeque::with_capacity(CAPACITY)))
    }

    pub fn entries(&self) -> Vec<LogEntry> {
        self.0.lock().unwrap().iter().cloned().collect()
    }

    pub fn clear(&self) {
        self.0.lock().unwrap().clear();
    }

    fn push(&self, entry: LogEntry) {
        let mut buffer = self.0.lock().unwrap();
        if buffer.len() >= CAPACITY {
            buffer.pop_front();
        }
        buffer.push_back(entry);
    }
}

/// Record an entry and stream it to any listening Developer panel.
pub fn log(app: &AppHandle, level: LogLevel, source: &str, message: impl Into<String>) {
    let entry = LogEntry {
        at: chrono::Utc::now().to_rfc3339(),
        level,
        source: source.to_string(),
        message: message.into(),
    };
    if let Some(buffer) = app.try_state::<DevLog>() {
        buffer.push(entry.clone());
    }
    let _ = app.emit(DEV_LOG_EVENT, entry);
}

pub fn info(app: &AppHandle, source: &str, message: impl Into<String>) {
    log(app, LogLevel::Info, source, message);
}

pub fn warn(app: &AppHandle, source: &str, message: impl Into<String>) {
    log(app, LogLevel::Warn, source, message);
}

pub fn error(app: &AppHandle, source: &str, message: impl Into<String>) {
    log(app, LogLevel::Error, source, message);
}

pub fn debug(app: &AppHandle, source: &str, message: impl Into<String>) {
    log(app, LogLevel::Debug, source, message);
}
