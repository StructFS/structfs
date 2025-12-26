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
                CommandResult::Help => {
                    io.write_output(Output::normal(commands::format_help()))?;
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
            mount_count: 4, // http + http_sync + sys + help
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
        format!("/{}", path.components.join("/"))
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
    use std::collections::VecDeque;

    struct MockHost {
        inputs: VecDeque<String>,
        signals: VecDeque<Signal>,
        outputs: Vec<Output>,
    }

    impl MockHost {
        fn with_inputs(inputs: Vec<&str>) -> Self {
            Self {
                inputs: inputs.into_iter().map(String::from).collect(),
                signals: VecDeque::new(),
                outputs: Vec::new(),
            }
        }

        fn with_signal(mut self, signal: Signal) -> Self {
            self.signals.push_back(signal);
            self
        }
    }

    impl IoHost for MockHost {
        fn wait_for_input(&mut self) -> Result<(), IoError> {
            Ok(())
        }

        fn read_input(&mut self) -> Result<Option<crate::io::InputLine>, IoError> {
            Ok(self
                .inputs
                .pop_front()
                .map(|line| crate::io::InputLine { line }))
        }

        fn read_signal(&mut self) -> Result<Option<Signal>, IoError> {
            Ok(self.signals.pop_front())
        }

        fn write_output(&mut self, output: Output) -> Result<(), IoError> {
            self.outputs.push(output);
            Ok(())
        }

        fn write_prompt(&mut self, _config: PromptConfig) -> Result<(), IoError> {
            Ok(())
        }
    }

    #[test]
    fn test_exit_command() {
        let mut core = ReplCore::new();
        let mut host = MockHost::with_inputs(vec!["exit"]);

        let result = core.run(&mut host);

        assert!(matches!(result, Ok(ExitReason::UserExit)));
        assert!(host.outputs.iter().any(|o| o.text.contains("Goodbye")));
    }

    #[test]
    fn test_eof_signal() {
        let mut core = ReplCore::new();
        let mut host = MockHost::with_inputs(vec![]).with_signal(Signal::Eof);

        let result = core.run(&mut host);

        assert!(matches!(result, Ok(ExitReason::Eof)));
    }

    #[test]
    fn test_read_sys_time() {
        let mut core = ReplCore::new();
        let mut host = MockHost::with_inputs(vec!["read /ctx/sys/time/now", "exit"]);

        let result = core.run(&mut host);

        assert!(matches!(result, Ok(ExitReason::UserExit)));
        // Should have output containing a timestamp
        assert!(host.outputs.iter().any(|o| o.text.contains("T")));
    }
}
