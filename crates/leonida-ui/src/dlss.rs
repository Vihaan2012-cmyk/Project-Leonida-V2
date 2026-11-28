//! What the launcher says about DLSS while DLSS is the chosen upscaler. Asking NGX needs a Vulkan device and takes a
//! second or two, so it is asked on a thread of its own and the window polls for the answer.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Checking,
    /// What `leonida config dlss` prints after "DLSS is ".
    Available(String),
    Unavailable(String),
}

type Ask = Arc<dyn Fn(Option<PathBuf>) -> Line + Send + Sync>;

/// The real question: the report of the `upscale` crate for this `dlss_path`.
fn ask_ngx(path: Option<PathBuf>) -> Line {
    let report = upscale::dlss::report(&upscale::dlss::DlssConfig {
        dlss_dll: path,
        ..Default::default()
    });
    if report.available {
        Line::Available(report.summary)
    } else {
        Line::Unavailable(report.summary)
    }
}

/// The key a question is asked for: the `dlss_path` the form holds.
type Key = Option<PathBuf>;

pub struct Probe {
    ask: Ask,
    wanted: Option<Key>,
    answered: Option<(Key, Line)>,
    flying: Option<(Key, Receiver<Line>)>,
}

impl Default for Probe {
    fn default() -> Self {
        Probe::with(Arc::new(ask_ngx))
    }
}

impl Probe {
    fn with(ask: Ask) -> Self {
        Probe {
            ask,
            wanted: None,
            answered: None,
            flying: None,
        }
    }

    /// Says what the form holds: `None` when DLSS is not the chosen upscaler, else its `dlss_path`. One question is
    /// in flight at a time, so typing a path does not start one per key.
    pub fn want(&mut self, wanted: Option<Key>) {
        self.wanted = wanted;
        self.start();
    }

    fn start(&mut self) {
        let Some(key) = self.wanted.clone() else {
            return;
        };
        if self.flying.is_some()
            || self
                .answered
                .as_ref()
                .is_some_and(|(asked, _)| *asked == key)
        {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ask = self.ask.clone();
        let path = key.clone();
        std::thread::spawn(move || {
            let _ = tx.send(ask(path));
        });
        self.flying = Some((key, rx));
    }

    /// Collects an answer that arrived; true when the line changed.
    pub fn poll(&mut self) -> bool {
        let Some((key, rx)) = &self.flying else {
            return false;
        };
        let line = match rx.try_recv() {
            Ok(line) => line,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                Line::Unavailable("unavailable: the check stopped unexpectedly".into())
            }
        };
        self.answered = Some((key.clone(), line));
        self.flying = None;
        self.start();
        true
    }

    /// The line to show: `None` when DLSS is not chosen, `Checking` until the answer for the current path is in.
    pub fn line(&self) -> Option<Line> {
        let wanted = self.wanted.as_ref()?;
        Some(match &self.answered {
            Some((key, line)) if key == wanted => line.clone(),
            _ => Line::Checking,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    fn wait(probe: &mut Probe) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !probe.poll() {
            assert!(Instant::now() < deadline, "no answer");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn recording() -> (Probe, Arc<Mutex<Vec<Key>>>) {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let log = asked.clone();
        let probe = Probe::with(Arc::new(move |path| {
            log.lock().unwrap().push(path.clone());
            match path {
                Some(_) => Line::Unavailable("unavailable: no file".into()),
                None => Line::Available("available on a GPU".into()),
            }
        }));
        (probe, asked)
    }

    #[test]
    fn nothing_is_asked_while_dlss_is_not_chosen() {
        let (mut probe, asked) = recording();
        probe.want(None);
        assert_eq!(probe.line(), None);
        assert!(!probe.poll());
        assert!(asked.lock().unwrap().is_empty());
    }

    #[test]
    fn the_answer_comes_back_off_the_calling_thread_and_is_kept() {
        let (mut probe, asked) = recording();
        probe.want(Some(None));
        assert_eq!(probe.line(), Some(Line::Checking));
        wait(&mut probe);
        assert_eq!(
            probe.line(),
            Some(Line::Available("available on a GPU".into()))
        );
        probe.want(Some(None));
        assert!(!probe.poll());
        assert_eq!(
            asked.lock().unwrap().len(),
            1,
            "the same path is not asked twice"
        );
    }

    #[test]
    fn a_changed_path_is_asked_again_after_the_one_in_flight() {
        let (mut probe, asked) = recording();
        probe.want(Some(None));
        probe.want(Some(Some("a.dll".into())));
        probe.want(Some(Some("b.dll".into())));
        assert_eq!(probe.line(), Some(Line::Checking));
        wait(&mut probe);
        assert_eq!(
            probe.line(),
            Some(Line::Checking),
            "the answer is for an old path"
        );
        wait(&mut probe);
        assert!(matches!(probe.line(), Some(Line::Unavailable(_))));
        let asked = asked.lock().unwrap();
        assert_eq!(
            *asked,
            vec![None, Some(PathBuf::from("b.dll"))],
            "only the first and the latest path were asked"
        );
    }

    #[test]
    #[ignore = "needs an NVIDIA GPU and nvngx_dlss.dll (set LEONIDA_DLSS_DLL)"]
    fn the_real_question_is_answered_with_the_report_text() {
        let mut probe = Probe::default();
        probe.want(Some(None));
        wait(&mut probe);
        match probe.line() {
            Some(Line::Available(summary)) => {
                assert!(summary.starts_with("available on "), "{summary}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn choosing_another_upscaler_hides_the_line() {
        let (mut probe, _) = recording();
        probe.want(Some(None));
        wait(&mut probe);
        probe.want(None);
        assert_eq!(probe.line(), None);
    }
}
