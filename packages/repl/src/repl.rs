//! Platform-independent REPL core.
//!
//! This module contains the main REPL loop logic.

use crate::commands::{self, CommandResult};
use crate::io::{ExitReason, IoError, IoHost, Output, PromptConfig, Signal};
use crate::store_context::StoreContext;

/// The platform-independent REPL core.
pub struct ReplCore {
    ctx: StoreContext,
}

impl ReplCore {
    /// Create a new REPL core with default stores.
    pub fn new() -> Self {
        Self {
            ctx: StoreContext::new(),
        }
    }

    /// Run the REPL loop, reading/writing through the provided I/O host.
    pub fn run(&mut self, io: &mut impl IoHost) -> Result<ExitReason, IoError> {
        self.write_banner(io)?;
        for warning in self.ctx.take_warnings() {
            io.write_output(Output::error(warning))?;
        }

        loop {
            self.update_prompt(io)?;
            io.wait_for_input()?;

            if let Some(signal) = io.read_signal()? {
                match signal {
                    Signal::Eof => {
                        io.write_output(Output::info("Goodbye!"))?;
                        io.flush()?;
                        return Ok(ExitReason::Eof);
                    }
                    Signal::Interrupt => {
                        io.write_output(Output::info("^C (use 'exit' to quit)"))?;
                        continue;
                    }
                }
            }

            let input = match io.read_input()? {
                Some(input) => input,
                None => continue,
            };

            let result = commands::execute(&input.line, &mut self.ctx);

            match result {
                CommandResult::Ok { display: None, .. } => {}
                CommandResult::Ok {
                    display: Some(output),
                    ..
                } => {
                    io.write_output(Output::normal(output))?;
                }
                CommandResult::Error(msg) => {
                    io.write_output(Output::error(msg))?;
                }
                CommandResult::Exit => {
                    io.write_output(Output::info("Goodbye!"))?;
                    io.flush()?;
                    return Ok(ExitReason::UserExit);
                }
            }

            io.flush()?;
        }
    }

    /// Get a reference to the store context.
    pub fn context(&self) -> &StoreContext {
        &self.ctx
    }

    /// Get a mutable reference to the store context.
    pub fn context_mut(&mut self) -> &mut StoreContext {
        &mut self.ctx
    }

    fn write_banner(&self, io: &mut impl IoHost) -> Result<(), IoError> {
        io.write_output(Output::banner(BANNER))
    }

    fn update_prompt(&self, io: &mut impl IoHost) -> Result<(), IoError> {
        let current_path = format_path(self.ctx.current_path());

        io.write_prompt(PromptConfig {
            mount_count: self.ctx.mount_count(),
            current_path,
        })
    }
}

impl Default for ReplCore {
    fn default() -> Self {
        Self::new()
    }
}

fn format_path(path: &structfs_core_store::Path) -> String {
    if path.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", path)
    }
}

const BANNER: &str = r#"
  _____ _                   _   _____ ____
 / ____| |                 | | |  ___/ ___|
| (___ | |_ _ __ _   _  ___| |_| |_  \___ \
 \___ \| __| '__| | | |/ __| __|  _|  ___) |
 ____) | |_| |  | |_| | (__| |_| |   |____/
|_____/ \__|_|   \__,_|\___|\___|_|

Type 'help' for available commands, 'exit' to quit.
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::TestHost;
    use crate::mounts::DEFAULT_MOUNTS;

    #[test]
    fn test_exit_command() {
        let mut core = ReplCore::new();
        let mut host = TestHost::new();
        host.queue_input("exit");

        let result = core.run(&mut host);

        assert!(matches!(result, Ok(ExitReason::UserExit)));
        assert!(host.output_text().contains("Goodbye"));
        assert!(host.errors().is_empty(), "{:?}", host.errors());
    }

    #[test]
    fn test_eof_signal() {
        let mut core = ReplCore::new();
        let mut host = TestHost::new();
        host.queue_signal(Signal::Eof);

        assert!(matches!(core.run(&mut host), Ok(ExitReason::Eof)));
    }

    #[test]
    fn test_interrupt_then_exit() {
        let mut core = ReplCore::new();
        let mut host = TestHost::new();
        host.queue_signal(Signal::Interrupt);
        host.queue_input("exit");

        assert!(matches!(core.run(&mut host), Ok(ExitReason::UserExit)));
        assert!(host.output_text().contains("^C"));
    }

    #[test]
    fn test_read_sys_time() {
        let mut core = ReplCore::new();
        let mut host = TestHost::new();
        host.queue_inputs(["read /ctx/sys/time/now", "exit"]);

        let result = core.run(&mut host);

        assert!(matches!(result, Ok(ExitReason::UserExit)));
        // Should have output containing a timestamp
        assert!(host.output_text().contains('T'));
    }

    #[test]
    fn errors_are_reported_as_errors() {
        let mut core = ReplCore::new();
        let mut host = TestHost::new();
        host.queue_inputs(["bogus", "exit"]);
        core.run(&mut host).unwrap();
        assert_eq!(host.errors().len(), 1);
    }

    #[test]
    fn prompt_counts_mounts_as_they_change() {
        let mut core = ReplCore::new();
        // The prompt is written before each line is read, so the last
        // prompt reflects every command before `exit`.
        let mut host = TestHost::new();
        host.queue_inputs(["write /ctx/mounts/a {\"type\": \"memory\"}", "exit"]);
        core.run(&mut host).unwrap();
        assert_eq!(
            host.last_prompt().unwrap().mount_count,
            DEFAULT_MOUNTS.len() + 1
        );

        let mut host = TestHost::new();
        host.queue_inputs(["write /ctx/mounts/a null", "cd /ctx", "exit"]);
        core.run(&mut host).unwrap();
        let prompt = host.last_prompt().unwrap();
        assert_eq!(prompt.mount_count, DEFAULT_MOUNTS.len());
        assert_eq!(prompt.current_path, "/ctx");
        assert_eq!(core.context().current_path().to_string(), "ctx");
        assert_eq!(core.context_mut().mount_count(), DEFAULT_MOUNTS.len());
    }
}
