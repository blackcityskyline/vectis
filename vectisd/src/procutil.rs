use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Runs cmd, kills if it doesn't exit within timeout.
/// Ok(None) = killed for timeout, Ok(Some(status)) = normal exit, Err = spawn failure.
pub fn run_with_timeout(cmd: &str, args: &[&str], timeout: Duration) -> std::io::Result<Option<ExitStatus>> {
    let mut child = Command::new(cmd).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;

    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            tracing::warn!("procutil: '{cmd} {}' exceeded {timeout:?}, killing it", args.join(" "));
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn run_shell_with_timeout(script: &str, timeout: Duration) -> std::io::Result<Option<ExitStatus>> {
    run_with_timeout("sh", &["-c", script], timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_command_returns_normally() {
        let result = run_with_timeout("true", &[], Duration::from_secs(5)).unwrap();
        assert!(result.is_some());
        assert!(result.unwrap().success());
    }

    #[test]
    fn hung_command_gets_killed_within_timeout() {
        let start = Instant::now();
        let result = run_with_timeout("sleep", &["30"], Duration::from_millis(300)).unwrap();
        assert!(result.is_none(), "expected timeout (None), got {result:?}");
        assert!(start.elapsed() < Duration::from_secs(2), "took too long to kill");
    }

    #[test]
    fn nonexistent_command_errors() {
        let result = run_with_timeout("this-binary-does-not-exist-xyz", &[], Duration::from_secs(5));
        assert!(result.is_err());
    }
}
