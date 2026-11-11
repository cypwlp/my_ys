//! Job registry: what the UI shows as the work queue.
//!
//! One running job at a time is a policy, not a limitation: two extractions into
//! the same folder fight each other, and a cancelled job must still report the
//! files it already wrote.  `parallel` raises the cap when the user asks for it.

use crate::cancel::CancelToken;
use crate::models::Progress;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum JobState {
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: u64,
    /// "extract" | "compress" | "test"
    pub kind: String,
    pub label: String,
    pub state: JobState,
    /// -1 when the engine cannot know the total.
    pub percent: f64,
    pub detail: String,
    pub items: usize,
    pub bytes: u64,
    pub blocked: usize,
    pub error: Option<String>,
}

#[derive(Default)]
struct Inner {
    order: Vec<u64>,
    jobs: HashMap<u64, Job>,
    tokens: HashMap<u64, CancelToken>,
    running: usize,
}

pub struct Jobs {
    inner: RwLock<Inner>,
    events: Mutex<Vec<(String, Job)>>,
    next: AtomicU64,
    parallel: usize,
}

impl Default for Jobs {
    fn default() -> Self {
        Self::new(1)
    }
}

impl Jobs {
    pub fn new(parallel: usize) -> Self {
        Self {
            inner: RwLock::new(Inner::default()),
            events: Mutex::new(Vec::new()),
            next: AtomicU64::new(1),
            parallel: parallel.max(1),
        }
    }

    /// Register and, if a slot is free, hand back the id plus its cancel token.
    /// `None` means the queue is full and the caller must not start the work.
    pub fn begin(&self, kind: &str, label: String, detail: String) -> Option<(u64, CancelToken)> {
        let mut inner = self.inner.write().ok()?;
        if inner.running >= self.parallel {
            // A refused job gets no id.  Ids are what the UI shows and cancels,
            // so burning one per rejection would make the numbering jump.
            return None;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let token = CancelToken::new();
        inner.running += 1;
        inner.order.push(id);
        inner.jobs.insert(
            id,
            Job {
                id,
                kind: kind.to_string(),
                label,
                state: JobState::Running,
                percent: -1.0,
                detail,
                items: 0,
                bytes: 0,
                blocked: 0,
                error: None,
            },
        );
        inner.tokens.insert(id, token.clone());
        Some((id, token))
    }

    pub fn report(&self, id: u64, p: &Progress) {
        if let Ok(mut inner) = self.inner.write() {
            if let Some(job) = inner.jobs.get_mut(&id) {
                job.percent = p.fraction.map(|f| (f * 100.0).round()).unwrap_or(-1.0);
                if !p.summary.is_empty() {
                    job.detail = p.summary.clone();
                }
            }
        }
        self.publish(id);
    }

    pub fn finish(&self, id: u64, state: JobState, items: usize, bytes: u64, blocked: usize, error: Option<String>) {
        if let Ok(mut inner) = self.inner.write() {
            if let Some(job) = inner.jobs.get_mut(&id) {
                job.state = state;
                job.items = items;
                job.bytes = bytes;
                job.blocked = blocked;
                job.error = error;
                if state != JobState::Running {
                    job.percent = if state == JobState::Done { 100.0 } else { job.percent };
                    inner.running = inner.running.saturating_sub(1);
                }
            }
            inner.tokens.remove(&id);
        }
        self.publish(id);
    }

    pub fn cancel(&self, id: u64) -> bool {
        let token = self.inner.read().ok().and_then(|i| i.tokens.get(&id).cloned());
        match token {
            Some(t) => {
                t.cancel();
                true
            }
            // Already finished: nothing to stop, but the UI asked, so say so.
            None => false,
        }
    }

    pub fn snapshot(&self) -> Vec<Job> {
        let inner = match self.inner.read() {
            Ok(i) => i,
            Err(_) => return Vec::new(),
        };
        inner.order.iter().filter_map(|id| inner.jobs.get(id).cloned()).collect()
    }

    /// Remove finished jobs; running ones are never dropped.
    pub fn clear_finished(&self) -> usize {
        let mut removed = 0;
        if let Ok(mut inner) = self.inner.write() {
            let done: Vec<u64> = inner
                .order
                .iter()
                .filter_map(|id| {
                    inner.jobs.get(id).filter(|j| j.state != JobState::Running).map(|j| j.id)
                })
                .collect();
            for id in done {
                inner.jobs.remove(&id);
                inner.order.retain(|x| *x != id);
                removed += 1;
            }
        }
        removed
    }

    /// Drain the events produced since the last call.  The UI polls this from a
    /// timer instead of the worker firing a Tauri event per file.
    pub fn take_events(&self) -> Vec<(String, Job)> {
        self.events.lock().map(|mut e| std::mem::take(&mut *e)).unwrap_or_default()
    }

    fn publish(&self, id: u64) {
        let job = self.inner.read().ok().and_then(|i| i.jobs.get(&id).cloned());
        if let Some(job) = job {
            if let Ok(mut events) = self.events.lock() {
                events.push((job.kind.clone(), job));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Overwrite;

    fn progress(f: Option<f64>, summary: &str) -> Progress {
        Progress { fraction: f, done: 0, total: 0, summary: summary.into() }
    }

    #[test]
    fn serial_queue_refuses_a_second_running_job() {
        let jobs = Jobs::default();
        let (a, _ta) = jobs.begin("extract", "A".into(), String::new()).unwrap();
        assert!(jobs.begin("extract", "B".into(), String::new()).is_none(), "cap is 1 by default");
        jobs.finish(a, JobState::Done, 3, 100, 0, None);
        let (b, _tb) = jobs.begin("extract", "B".into(), String::new()).unwrap();
        assert_eq!(b, 2);
        let snap = jobs.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].state, JobState::Done);
    }

    #[test]
    fn parallel_queue_allows_more_than_one() {
        let jobs = Jobs::new(3);
        assert!(jobs.begin("compress", "A".into(), String::new()).is_some());
        assert!(jobs.begin("compress", "B".into(), String::new()).is_some());
        assert!(jobs.begin("compress", "C".into(), String::new()).is_some());
        assert!(jobs.begin("compress", "D".into(), String::new()).is_none());
    }

    #[test]
    fn cancel_sets_the_token_and_reports_whether_it_was_live() {
        let jobs = Jobs::default();
        let (id, token) = jobs.begin("extract", "A".into(), String::new()).unwrap();
        assert!(!token.cancelled());
        assert!(jobs.cancel(id));
        assert!(token.cancelled(), "the worker must observe the flag");
        jobs.finish(id, JobState::Cancelled, 1, 10, 0, None);
        assert!(!jobs.cancel(id), "a finished job has nothing to cancel");
        assert_eq!(jobs.snapshot()[0].state, JobState::Cancelled);
    }

    #[test]
    fn progress_updates_percent_and_detail() {
        let jobs = Jobs::default();
        let (id, _t) = jobs.begin("extract", "A".into(), "start".into()).unwrap();
        jobs.report(id, &progress(Some(0.42), "docs/report.txt"));
        let snap = jobs.snapshot();
        assert_eq!(snap[0].percent, 42.0);
        assert_eq!(snap[0].detail, "docs/report.txt");

        jobs.report(id, &progress(None, ""));
        assert_eq!(jobs.snapshot()[0].percent, -1.0, "unknown total is -1, never 0");
        assert_eq!(jobs.snapshot()[0].detail, "docs/report.txt", "empty summary keeps the last one");
    }

    #[test]
    fn events_are_drained_once_so_the_ui_cannot_see_a_job_twice() {
        let jobs = Jobs::default();
        let (id, _t) = jobs.begin("extract", "A".into(), String::new()).unwrap();
        jobs.report(id, &progress(Some(0.5), "x"));
        let first = jobs.take_events();
        assert_eq!(first.len(), 1);
        assert!(jobs.take_events().is_empty(), "second poll must be empty");
        jobs.finish(id, JobState::Done, 2, 20, 1, None);
        let second = jobs.take_events();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].1.state, JobState::Done);
        assert_eq!(second[0].1.blocked, 1);
    }

    #[test]
    fn clear_finished_keeps_running_jobs_and_reports_how_many_it_dropped() {
        let jobs = Jobs::new(2);
        let (a, _ta) = jobs.begin("extract", "A".into(), String::new()).unwrap();
        let (b, _tb) = jobs.begin("extract", "B".into(), String::new()).unwrap();
        jobs.finish(a, JobState::Done, 1, 1, 0, None);
        assert_eq!(jobs.clear_finished(), 1);
        let snap = jobs.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].id, b, "the running job must survive");
    }

    #[test]
    fn failure_carries_the_message_the_user_needs() {
        let jobs = Jobs::default();
        let (id, _t) = jobs.begin("test", "A".into(), String::new()).unwrap();
        jobs.finish(id, JobState::Failed, 0, 0, 0, Some("資料錯誤（檔頭或資料區損毀）".into()));
        let snap = jobs.snapshot();
        assert_eq!(snap[0].state, JobState::Failed);
        assert!(snap[0].error.as_deref().unwrap().contains("損毀"));
    }

    #[test]
    fn overwrite_policy_is_part_of_the_request_not_the_queue() {
        // Guards the enum mapping the 7z adapter relies on; a wrong code here
        // silently changes what gets overwritten.
        assert_eq!(Overwrite::Always.sevenzip_code(), Some('s'));
        assert_eq!(Overwrite::Skip.sevenzip_code(), Some('t'));
        assert_eq!(Overwrite::KeepBoth.sevenzip_code(), Some('u'));
        assert_eq!(Overwrite::Ask.sevenzip_code(), None);
        assert_eq!(Overwrite::Newer.sevenzip_code(), None);
    }
}
