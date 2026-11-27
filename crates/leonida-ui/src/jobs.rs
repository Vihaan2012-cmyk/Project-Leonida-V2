use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

pub type Wake = Arc<dyn Fn() + Send + Sync>;

pub enum Outcome {
    GamesFound(Vec<PathBuf>),
    /// A command run as a child process ended.
    Command(CommandResult),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    pub name: String,
    pub success: bool,
    pub detail: String,
}

enum Event {
    Progress {
        fraction: Option<f32>,
        message: String,
    },
    Log(String),
    Finished(Outcome),
}

pub struct Reporter {
    tx: Sender<Event>,
    cancel: Arc<AtomicBool>,
    wake: Wake,
}

impl Reporter {
    pub fn progress(&self, fraction: f32, message: impl Into<String>) {
        let fraction = fraction.clamp(0.0, 1.0);
        let _ = self.tx.send(Event::Progress {
            fraction: Some(fraction),
            message: message.into(),
        });
        (self.wake)();
    }

    /// Work that cannot say how far along it is.
    pub fn busy(&self, message: impl Into<String>) {
        let _ = self.tx.send(Event::Progress {
            fraction: None,
            message: message.into(),
        });
        (self.wake)();
    }

    pub fn log(&self, message: impl Into<String>) {
        let _ = self.tx.send(Event::Log(message.into()));
        (self.wake)();
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

pub struct Running {
    pub title: String,
    /// `None` while the work cannot say how far along it is.
    pub fraction: Option<f32>,
    pub message: String,
    cancel: Arc<AtomicBool>,
    rx: Receiver<Event>,
}

impl Running {
    #[cfg(test)]
    pub fn cancelling(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

#[derive(Default)]
pub struct Polled {
    pub logs: Vec<String>,
    pub finished: Option<Outcome>,
}

#[derive(Default)]
pub struct Jobs {
    running: Option<Running>,
}

impl Jobs {
    pub fn running(&self) -> Option<&Running> {
        self.running.as_ref()
    }

    pub fn is_busy(&self) -> bool {
        self.running.is_some()
    }

    pub fn start(
        &mut self,
        title: impl Into<String>,
        wake: Wake,
        work: impl FnOnce(&Reporter) -> Outcome + Send + 'static,
    ) -> bool {
        if self.is_busy() {
            return false;
        }
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let reporter = Reporter {
            tx: tx.clone(),
            cancel: cancel.clone(),
            wake: wake.clone(),
        };
        thread::spawn(move || {
            let outcome = work(&reporter);
            let _ = tx.send(Event::Finished(outcome));
            wake();
        });
        self.running = Some(Running {
            title: title.into(),
            fraction: Some(0.0),
            message: String::new(),
            cancel,
            rx,
        });
        true
    }

    pub fn cancel(&self) {
        if let Some(running) = &self.running {
            running.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn poll(&mut self) -> Polled {
        let mut polled = Polled::default();
        let mut done = false;
        if let Some(running) = &mut self.running {
            loop {
                match running.rx.try_recv() {
                    Ok(Event::Progress { fraction, message }) => {
                        running.fraction = fraction;
                        running.message = message;
                    }
                    Ok(Event::Log(line)) => polled.logs.push(line),
                    Ok(Event::Finished(outcome)) => {
                        polled.finished = Some(outcome);
                        done = true;
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        polled
                            .logs
                            .push(format!("{} stopped unexpectedly", running.title));
                        done = true;
                        break;
                    }
                }
            }
        }
        if done {
            self.running = None;
        }
        polled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn no_wake() -> Wake {
        Arc::new(|| {})
    }

    fn finish(jobs: &mut Jobs) -> (Vec<String>, Outcome) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut logs = Vec::new();
        loop {
            let polled = jobs.poll();
            logs.extend(polled.logs);
            if let Some(outcome) = polled.finished {
                return (logs, outcome);
            }
            assert!(Instant::now() < deadline, "job did not finish");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn runs_one_job_and_collects_its_logs() {
        let mut jobs = Jobs::default();
        assert!(jobs.start("test", no_wake(), |r| {
            r.progress(0.5, "half");
            r.log("hello");
            Outcome::GamesFound(vec![PathBuf::from("x")])
        }));
        assert!(jobs.is_busy());
        let (logs, outcome) = finish(&mut jobs);
        assert_eq!(logs, vec!["hello".to_string()]);
        assert!(matches!(outcome, Outcome::GamesFound(paths) if paths == vec![PathBuf::from("x")]));
        assert!(!jobs.is_busy());
    }

    #[test]
    fn second_job_is_refused_while_busy() {
        let mut jobs = Jobs::default();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        jobs.start("first", no_wake(), move |_| {
            let _ = release_rx.recv();
            Outcome::GamesFound(Vec::new())
        });
        assert!(!jobs.start("second", no_wake(), |_| Outcome::GamesFound(Vec::new())));
        release_tx.send(()).unwrap();
        finish(&mut jobs);
    }

    #[test]
    fn cancel_reaches_the_worker() {
        let mut jobs = Jobs::default();
        jobs.start("loop", no_wake(), |r| {
            while !r.cancelled() {
                thread::sleep(Duration::from_millis(1));
            }
            r.log("stopped");
            Outcome::GamesFound(Vec::new())
        });
        jobs.cancel();
        assert!(jobs.running().unwrap().cancelling());
        let (logs, _) = finish(&mut jobs);
        assert_eq!(logs, vec!["stopped".to_string()]);
    }

    #[test]
    fn a_panicking_job_is_reported() {
        let mut jobs = Jobs::default();
        jobs.start("boom", no_wake(), |_| panic!("test panic"));
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut logs = Vec::new();
        while jobs.is_busy() {
            logs.extend(jobs.poll().logs);
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(logs, vec!["boom stopped unexpectedly".to_string()]);
    }
}
