// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use crate::Result;
use std::{
    io::Read,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Debug)]
pub struct ChildFailure(pub i32, pub String);
impl std::fmt::Display for ChildFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (exit {})", self.1, self.0)
    }
}
impl std::error::Error for ChildFailure {}

pub struct Runner {
    stopping: Arc<AtomicBool>,
}
impl Runner {
    pub fn new() -> Result<Self> {
        let stopping = Arc::new(AtomicBool::new(false));
        let flag = stopping.clone();
        ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst))?;
        Ok(Self { stopping })
    }
    pub fn interrupted(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }
    pub fn run(&self, command: &mut Command) -> Result<()> {
        self.execute(command, false).map(|_| ())
    }
    pub fn capture(&self, command: &mut Command) -> Result<String> {
        Ok(String::from_utf8(self.execute(command, true)?)?
            .trim()
            .to_owned())
    }
    fn execute(&self, command: &mut Command, capture: bool) -> Result<Vec<u8>> {
        if self.interrupted() {
            return Err(Box::new(ChildFailure(130, "interrupted".into())));
        }
        eprintln!("> {command:?}");
        command
            .stdin(if capture {
                Stdio::null()
            } else {
                Stdio::inherit()
            })
            .stderr(Stdio::inherit())
            .stdout(if capture {
                Stdio::piped()
            } else {
                Stdio::inherit()
            });
        // Forward cancellation to the complete native task, including Cargo's compiler children.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn()?;
        let output = child.stdout.take().map(|mut pipe| {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                pipe.read_to_end(&mut bytes).map(|_| bytes)
            })
        });
        let mut cancelled_at = None;
        #[cfg(unix)]
        let mut forced = false;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if self.interrupted() {
                #[cfg(unix)]
                {
                    use nix::{
                        sys::signal::{Signal, killpg},
                        unistd::Pid,
                    };
                    if cancelled_at.is_none() {
                        let _ = killpg(Pid::from_raw(child.id().try_into()?), Signal::SIGTERM);
                    } else if !forced
                        && cancelled_at
                            .is_some_and(|t: Instant| t.elapsed() >= Duration::from_secs(2))
                    {
                        let _ = killpg(Pid::from_raw(child.id().try_into()?), Signal::SIGKILL);
                        forced = true;
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = child.kill();
                }
                cancelled_at.get_or_insert_with(Instant::now);
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let bytes = match output {
            Some(thread) => thread
                .join()
                .map_err(|_| "child stdout reader panicked")??,
            None => Vec::new(),
        };
        if self.interrupted() {
            return Err(Box::new(ChildFailure(130, "interrupted".into())));
        }
        if !status.success() {
            #[cfg(unix)]
            let code = {
                use std::os::unix::process::ExitStatusExt;
                status.code().unwrap_or(128 + status.signal().unwrap_or(1))
            };
            #[cfg(not(unix))]
            let code = status.code().unwrap_or(1);
            return Err(Box::new(ChildFailure(code, format!("{command:?}"))));
        }
        Ok(bytes)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn preserves_failure_status_drains_output_and_cancels_children() {
        let runner = Runner::new().unwrap();
        let error = runner
            .run(Command::new("sh").args(["-c", "exit 17"]))
            .unwrap_err();
        assert_eq!(error.downcast_ref::<ChildFailure>().unwrap().0, 17);
        let output = runner
            .capture(Command::new("sh").args(["-c", "printf 'captured stdout'"]))
            .unwrap();
        assert_eq!(output, "captured stdout");
        let flag = runner.stopping.clone();
        let trigger = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            flag.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let error = runner
            .run(Command::new("sh").args(["-c", "sleep 60"]))
            .unwrap_err();
        trigger.join().unwrap();
        assert_eq!(error.downcast_ref::<ChildFailure>().unwrap().0, 130);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(
            runner
                .run(Command::new("sh").args(["-c", "exit 0"]))
                .is_err()
        );
    }
}
