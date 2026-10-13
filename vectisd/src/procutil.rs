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

/// Runs `sh -c script`, captures combined stdout+stderr.
/// Ok(None) = killed for timeout, Ok(Some(output)) = finished, Err = spawn failure.
pub fn run_shell_capture(script: &str, timeout: Duration) -> std::io::Result<Option<String>> {
    use std::io::Read;

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let out_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(r) = out.as_mut() {
            let _ = r.read_to_string(&mut buf);
        }
        buf
    });
    let err_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(r) = err.as_mut() {
            let _ = r.read_to_string(&mut buf);
        }
        buf
    });

    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait()?.is_some() {
            let mut output = out_thread.join().unwrap_or_default();
            output.push_str(&err_thread.join().unwrap_or_default());
            return Ok(Some(output));
        }
        if Instant::now() >= deadline {
            tracing::warn!("procutil: 'sh -c {script}' exceeded {timeout:?}, killing it");
            let _ = child.kill();
            let _ = child.wait();
            let _ = out_thread.join();
            let _ = err_thread.join();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
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

    #[test]
    fn shell_capture_returns_output() {
        let out = run_shell_capture("echo stdout; echo stderr >&2", Duration::from_secs(5))
            .unwrap()
            .expect("should not time out");
        assert!(out.contains("stdout"));
        assert!(out.contains("stderr"));
    }

    #[test]
    fn shell_capture_times_out() {
        let start = Instant::now();
        let result = run_shell_capture("sleep 30", Duration::from_millis(300)).unwrap();
        assert!(result.is_none(), "expected timeout, got {result:?}");
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
