//! Running a Leonida command as a child process and following what it prints.

use std::io::{BufRead, BufReader, Read};
use std::process::Stdio;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::jobs::{CommandResult, Reporter};
use crate::play::Launch;

/// How far along a line says the work is: `12/340`, `12 of 340` or `42%`, with the rest of the line as the message.
pub fn progress_of(line: &str) -> Option<f32> {
    let words: Vec<&str> = line.split_whitespace().collect();
    for (at, word) in words.iter().enumerate() {
        if let Some(percent) = word.strip_suffix('%')
            && let Ok(percent) = percent.parse::<f32>()
            && (0.0..=100.0).contains(&percent)
        {
            return Some(percent / 100.0);
        }
        let pair = if let Some((done, total)) = word.split_once('/') {
            Some((done, total))
        } else if words.get(at + 1) == Some(&"of") {
            words.get(at + 2).map(|total| (*word, *total))
        } else {
            None
        };
        if let Some((done, total)) = pair
            && let (Ok(done), Ok(total)) = (
                done.trim_matches(['[', '(']).parse::<f32>(),
                total.trim_matches([']', ')', ':', ',']).parse::<f32>(),
            )
            && total > 0.0
            && done <= total
        {
            return Some(done / total);
        }
    }
    None
}

fn read_lines(stream: impl Read + Send + 'static, tx: mpsc::Sender<String>) {
    thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
}

/// Runs `launch` to its end, sending its output lines to the log and its progress to the bar. Stops it when the
/// job is cancelled.
pub fn run(launch: &Launch, name: &str, reporter: &Reporter) -> CommandResult {
    let failed = |detail: String| CommandResult {
        name: name.to_string(),
        success: false,
        detail,
    };
    let mut command = launch.command();
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return failed(format!("could not start {}: {error}", launch.describe())),
    };
    let (tx, rx) = mpsc::channel();
    if let Some(stdout) = child.stdout.take() {
        read_lines(stdout, tx.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        read_lines(stderr, tx.clone());
    }
    drop(tx);
    reporter.busy(format!("{name}..."));
    let mut stopped = false;
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                match progress_of(&line) {
                    Some(fraction) => reporter.progress(fraction, line.trim().to_string()),
                    None => reporter.busy(line.trim().to_string()),
                }
                reporter.log(line);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if reporter.cancelled() && !stopped {
                    stopped = true;
                    let _ = child.kill();
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    match child.wait() {
        _ if stopped => failed("cancelled".to_string()),
        Ok(status) if status.success() => CommandResult {
            name: name.to_string(),
            success: true,
            detail: String::new(),
        },
        Ok(status) => failed(match status.code() {
            Some(code) => format!("ended with code {code}"),
            None => "ended without a code".to_string(),
        }),
        Err(error) => failed(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{Jobs, Outcome};
    use std::ffi::OsString;
    use std::time::Instant;

    #[test]
    fn progress_is_read_from_counts_and_percentages() {
        assert_eq!(progress_of("translated 50/200 programs"), Some(0.25));
        assert_eq!(progress_of("[3 of 12] building"), Some(0.25));
        assert_eq!(progress_of("built 3 of 12"), Some(0.25));
        assert_eq!(progress_of("42% done"), Some(0.42));
        assert_eq!(progress_of("100%"), Some(1.0));
        assert_eq!(progress_of("pipeline cache: 12 KiB loaded"), None);
        assert_eq!(progress_of("took 5.0 s"), None);
        assert_eq!(progress_of("5/0"), None);
        assert_eq!(progress_of("9/3"), None);
        assert_eq!(progress_of("150%"), None);
        assert_eq!(progress_of("src/lib.rs failed"), None);
    }

    fn run_to_end(launch: Launch) -> (Vec<String>, CommandResult) {
        let mut jobs = Jobs::default();
        assert!(
            jobs.start("test", std::sync::Arc::new(|| {}), move |reporter| {
                Outcome::Command(run(&launch, "test", reporter))
            })
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut logs = Vec::new();
        loop {
            let polled = jobs.poll();
            logs.extend(polled.logs);
            if let Some(Outcome::Command(result)) = polled.finished {
                return (logs, result);
            }
            assert!(Instant::now() < deadline, "the command did not end");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_program_that_cannot_start_is_reported() {
        let launch = Launch {
            program: "E:/no/such/program".into(),
            args: Vec::new(),
        };
        let (_, result) = run_to_end(launch);
        assert!(!result.success);
        assert!(result.detail.starts_with("could not start"), "{result:?}");
    }

    #[test]
    fn the_output_of_a_command_reaches_the_log_and_its_end_is_reported() {
        let launch = Launch {
            program: std::env::current_exe().unwrap(),
            args: vec![OsString::from("--list"), OsString::from("--format=terse")],
        };
        let (logs, result) = run_to_end(launch);
        assert!(result.success, "{result:?}");
        assert!(logs.iter().any(|line| line.ends_with(": test")), "{logs:?}");
    }

    #[test]
    fn a_command_that_ends_badly_reports_its_code() {
        let launch = Launch {
            program: std::env::current_exe().unwrap(),
            args: vec![OsString::from("--no-such-option")],
        };
        let (_, result) = run_to_end(launch);
        assert!(!result.success);
        assert!(result.detail.starts_with("ended with code"), "{result:?}");
    }
}
