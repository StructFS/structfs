//! Test host implementation for in-memory I/O testing.
//!
//! This module provides a test implementation of the `IoHost` trait that uses
//! in-memory buffers instead of a real terminal. This enables testing the REPL
//! loop without requiring terminal interaction.

use std::collections::VecDeque;

use super::{InputLine, IoError, IoHost, Output, OutputStyle, PromptConfig, Signal};

/// Test host with in-memory I/O buffers.
///
/// Use this for testing the REPL without a terminal.
/// Input lines and signals are queued and consumed in order.
/// Output is buffered for later inspection.
#[derive(Debug, Default)]
pub struct TestHost {
    /// Queue of input lines to be returned by `read_input()`.
    input_queue: VecDeque<String>,
    /// Queue of signals to be returned by `read_signal()`.
    signal_queue: VecDeque<Signal>,
    /// Buffer of all output written via `write_output()`.
    output_buffer: Vec<Output>,
    /// The most recent prompt configuration.
    last_prompt: Option<PromptConfig>,
    /// Number of times `flush()` was called.
    flush_count: usize,
}

impl TestHost {
    /// Create a new empty test host.
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue an input line to be returned by `read_input()`.
    pub fn queue_input(&mut self, line: impl Into<String>) {
        self.input_queue.push_back(line.into());
    }

    /// Queue multiple input lines.
    pub fn queue_inputs(&mut self, lines: impl IntoIterator<Item = impl Into<String>>) {
        for line in lines {
            self.queue_input(line);
        }
    }

    /// Queue a signal to be returned by `read_signal()`.
    pub fn queue_signal(&mut self, signal: Signal) {
        self.signal_queue.push_back(signal);
    }

    /// Get all output that was written.
    pub fn output(&self) -> &[Output] {
        &self.output_buffer
    }

    /// Get output text only, joined with newlines.
    pub fn output_text(&self) -> String {
        self.output_buffer
            .iter()
            .map(|o| o.text.as_str())
            .collect::<Vec<_>>()
            .join("")
    }

    /// Get output of a specific style.
    pub fn output_with_style(&self, style: OutputStyle) -> Vec<&str> {
        self.output_buffer
            .iter()
            .filter(|o| o.style == style)
            .map(|o| o.text.as_str())
            .collect()
    }

    /// Get all error output.
    pub fn errors(&self) -> Vec<&str> {
        self.output_with_style(OutputStyle::Error)
    }

    /// Get the last prompt configuration, if any.
    pub fn last_prompt(&self) -> Option<&PromptConfig> {
        self.last_prompt.as_ref()
    }

    /// Get the number of times `flush()` was called.
    pub fn flush_count(&self) -> usize {
        self.flush_count
    }

    /// Clear the output buffer.
    pub fn clear_output(&mut self) {
        self.output_buffer.clear();
    }

    /// Check if there are pending inputs.
    pub fn has_pending_input(&self) -> bool {
        !self.input_queue.is_empty()
    }

    /// Check if there are pending signals.
    pub fn has_pending_signal(&self) -> bool {
        !self.signal_queue.is_empty()
    }
}

impl IoHost for TestHost {
    fn wait_for_input(&mut self) -> Result<(), IoError> {
        // In test mode, we don't actually wait - just return immediately.
        // The caller should have queued inputs before calling.
        Ok(())
    }

    fn read_input(&mut self) -> Result<Option<InputLine>, IoError> {
        Ok(self.input_queue.pop_front().map(|line| InputLine { line }))
    }

    fn read_signal(&mut self) -> Result<Option<Signal>, IoError> {
        Ok(self.signal_queue.pop_front())
    }

    fn write_output(&mut self, output: Output) -> Result<(), IoError> {
        self.output_buffer.push(output);
        Ok(())
    }

    fn write_prompt(&mut self, config: PromptConfig) -> Result<(), IoError> {
        self.last_prompt = Some(config);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        self.flush_count += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_empty_host() {
        let host = TestHost::new();
        assert!(host.input_queue.is_empty());
        assert!(host.signal_queue.is_empty());
        assert!(host.output_buffer.is_empty());
        assert!(host.last_prompt.is_none());
        assert_eq!(host.flush_count, 0);
    }

    #[test]
    fn default_creates_empty_host() {
        let host: TestHost = Default::default();
        assert!(!host.has_pending_input());
        assert!(!host.has_pending_signal());
    }

    #[test]
    fn queue_input_adds_to_queue() {
        let mut host = TestHost::new();
        host.queue_input("read /path");
        assert!(host.has_pending_input());
    }

    #[test]
    fn queue_inputs_adds_multiple() {
        let mut host = TestHost::new();
        host.queue_inputs(["cmd1", "cmd2", "cmd3"]);
        assert_eq!(host.input_queue.len(), 3);
    }

    #[test]
    fn queue_signal_adds_to_queue() {
        let mut host = TestHost::new();
        host.queue_signal(Signal::Interrupt);
        assert!(host.has_pending_signal());
    }

    #[test]
    fn read_input_returns_queued_in_order() {
        let mut host = TestHost::new();
        host.queue_input("first");
        host.queue_input("second");

        let first = host.read_input().unwrap();
        assert_eq!(first.unwrap().line, "first");

        let second = host.read_input().unwrap();
        assert_eq!(second.unwrap().line, "second");

        let third = host.read_input().unwrap();
        assert!(third.is_none());
    }

    #[test]
    fn read_signal_returns_queued_in_order() {
        let mut host = TestHost::new();
        host.queue_signal(Signal::Interrupt);
        host.queue_signal(Signal::Eof);

        let first = host.read_signal().unwrap();
        assert!(matches!(first, Some(Signal::Interrupt)));

        let second = host.read_signal().unwrap();
        assert!(matches!(second, Some(Signal::Eof)));

        let third = host.read_signal().unwrap();
        assert!(third.is_none());
    }

    #[test]
    fn wait_for_input_succeeds() {
        let mut host = TestHost::new();
        assert!(host.wait_for_input().is_ok());
    }

    #[test]
    fn write_output_buffers() {
        let mut host = TestHost::new();
        host.write_output(Output::normal("hello")).unwrap();
        host.write_output(Output::error("oops")).unwrap();

        assert_eq!(host.output().len(), 2);
        assert_eq!(host.output()[0].text, "hello");
        assert_eq!(host.output()[1].text, "oops");
    }

    #[test]
    fn output_text_joins() {
        let mut host = TestHost::new();
        host.write_output(Output::normal("line1\n")).unwrap();
        host.write_output(Output::normal("line2\n")).unwrap();

        assert_eq!(host.output_text(), "line1\nline2\n");
    }

    #[test]
    fn output_with_style_filters() {
        let mut host = TestHost::new();
        host.write_output(Output::normal("normal")).unwrap();
        host.write_output(Output::error("error")).unwrap();
        host.write_output(Output::info("info")).unwrap();

        let errors = host.output_with_style(OutputStyle::Error);
        assert_eq!(errors, vec!["error"]);
    }

    #[test]
    fn errors_returns_error_outputs() {
        let mut host = TestHost::new();
        host.write_output(Output::normal("ok")).unwrap();
        host.write_output(Output::error("error1")).unwrap();
        host.write_output(Output::error("error2")).unwrap();

        let errors = host.errors();
        assert_eq!(errors, vec!["error1", "error2"]);
    }

    #[test]
    fn write_prompt_stores_config() {
        let mut host = TestHost::new();
        assert!(host.last_prompt().is_none());

        host.write_prompt(PromptConfig {
            mount_count: 5,
            current_path: "/ctx".to_string(),
        })
        .unwrap();

        let prompt = host.last_prompt().unwrap();
        assert_eq!(prompt.mount_count, 5);
        assert_eq!(prompt.current_path, "/ctx");
    }

    #[test]
    fn flush_increments_counter() {
        let mut host = TestHost::new();
        assert_eq!(host.flush_count(), 0);

        host.flush().unwrap();
        assert_eq!(host.flush_count(), 1);

        host.flush().unwrap();
        host.flush().unwrap();
        assert_eq!(host.flush_count(), 3);
    }

    #[test]
    fn clear_output_empties_buffer() {
        let mut host = TestHost::new();
        host.write_output(Output::normal("text")).unwrap();
        assert!(!host.output().is_empty());

        host.clear_output();
        assert!(host.output().is_empty());
    }

    #[test]
    fn has_pending_input_after_consume() {
        let mut host = TestHost::new();
        host.queue_input("test");
        assert!(host.has_pending_input());

        host.read_input().unwrap();
        assert!(!host.has_pending_input());
    }

    #[test]
    fn has_pending_signal_after_consume() {
        let mut host = TestHost::new();
        host.queue_signal(Signal::Eof);
        assert!(host.has_pending_signal());

        host.read_signal().unwrap();
        assert!(!host.has_pending_signal());
    }

    #[test]
    fn output_banner_style() {
        let mut host = TestHost::new();
        host.write_output(Output::banner("Welcome!")).unwrap();

        let banners = host.output_with_style(OutputStyle::Banner);
        assert_eq!(banners, vec!["Welcome!"]);
    }

    #[test]
    fn output_info_style() {
        let mut host = TestHost::new();
        host.write_output(Output::info("Info message")).unwrap();

        let infos = host.output_with_style(OutputStyle::Info);
        assert_eq!(infos, vec!["Info message"]);
    }
}
