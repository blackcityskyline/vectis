use powerctl_common::error::{Error, Result};
use powerctl_common::types::Profile;
use std::time::Duration;

fn profile_to_tlp_subcommand(profile: Profile) -> &'static str {
    match profile {
        Profile::Powersave => "power-saver",
        Profile::Balanced => "balanced",
        Profile::Performance => "performance",
    }
}

pub trait CommandRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<()>;
}

const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);

pub struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<()> {
        match crate::procutil::run_with_timeout(program, args, COMMAND_TIMEOUT) {
            Ok(Some(status)) if status.success() => Ok(()),
            Ok(Some(status)) => Err(Error::Other(format!("{program} {} failed (exit {status})", args.join(" ")))),
            Ok(None) => Err(Error::Other(format!("{program} {} timed out after {COMMAND_TIMEOUT:?}", args.join(" ")))),
            Err(e) => Err(Error::Other(format!("failed to spawn {program}: {e}"))),
        }
    }
}

impl<T: CommandRunner> CommandRunner for &T {
    fn run(&self, program: &str, args: &[&str]) -> Result<()> {
        (**self).run(program, args)
    }
}

pub struct TlpRunner<R: CommandRunner> {
    runner: R,
}

impl<R: CommandRunner> TlpRunner<R> {
    pub fn new(runner: R) -> Self {
        Self { runner }
    }

    pub fn apply_profile(&self, profile: Profile) -> Result<()> {
        self.runner.run("tlp", &[profile_to_tlp_subcommand(profile)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeRunner {
        calls: RefCell<Vec<String>>,
        fail_on: Option<String>,
    }

    impl FakeRunner {
        fn new() -> Self {
            Self { calls: RefCell::new(Vec::new()), fail_on: None }
        }
        fn failing() -> Self {
            Self { calls: RefCell::new(Vec::new()), fail_on: Some("tlp".to_string()) }
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, program: &str, args: &[&str]) -> Result<()> {
            self.calls.borrow_mut().push(format!("{program} {}", args.join(" ")));
            if self.fail_on.as_deref() == Some(program) {
                return Err(Error::Other("simulated failure".to_string()));
            }
            Ok(())
        }
    }

    #[test]
    fn performance_profile_runs_tlp_performance() {
        let runner = FakeRunner::new();
        TlpRunner::new(&runner).apply_profile(Profile::Performance).unwrap();
        assert_eq!(*runner.calls.borrow(), vec!["tlp performance".to_string()]);
    }

    #[test]
    fn balanced_profile_runs_tlp_balanced() {
        let runner = FakeRunner::new();
        TlpRunner::new(&runner).apply_profile(Profile::Balanced).unwrap();
        assert_eq!(*runner.calls.borrow(), vec!["tlp balanced".to_string()]);
    }

    #[test]
    fn powersave_profile_runs_tlp_power_saver() {
        let runner = FakeRunner::new();
        TlpRunner::new(&runner).apply_profile(Profile::Powersave).unwrap();
        assert_eq!(*runner.calls.borrow(), vec!["tlp power-saver".to_string()]);
    }

    #[test]
    fn command_failure_propagates() {
        let runner = FakeRunner::failing();
        let result = TlpRunner::new(&runner).apply_profile(Profile::Balanced);
        assert!(result.is_err());
    }
}
