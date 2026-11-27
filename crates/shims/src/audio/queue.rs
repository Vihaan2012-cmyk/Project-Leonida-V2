//! The command queue and result queue behind `XApuEnqueueCommand`, `XApuDequeueResult` and
//! `XApuGetResultCount`. The command and result layouts are unmeasured, so a command is kept as the first bytes
//! the title built and completes at once with a zero status.

use std::collections::VecDeque;

/// Results kept before the oldest is dropped.
pub const RESULT_LIMIT: usize = 4096;
/// Bytes of a command kept for inspection.
pub const COMMAND_BYTES: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub sequence: u64,
    pub head: [u8; COMMAND_BYTES],
    pub flags: u64,
}

impl Command {
    /// The first word of the command, which is where an opcode would be.
    pub fn first_word(&self) -> u32 {
        u32::from_le_bytes([self.head[0], self.head[1], self.head[2], self.head[3]])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandResult {
    pub sequence: u64,
    pub status: u32,
}

#[derive(Debug, Default)]
pub struct CommandQueue {
    handle: Option<u64>,
    sequence: u64,
    commands: Vec<Command>,
    results: VecDeque<CommandResult>,
    dropped: u64,
}

/// What a connection hands back: positive and non-zero, so it reads as valid whether the title tests it for
/// zero or for a negative value.
const HANDLE: u64 = 0x4150_5500;

impl CommandQueue {
    pub fn connect(&mut self) -> u64 {
        *self.handle.get_or_insert(HANDLE)
    }

    pub fn disconnect(&mut self) {
        self.handle = None;
        self.results.clear();
    }

    pub fn connected(&self) -> bool {
        self.handle.is_some()
    }

    /// Takes a command; its result is ready at once. Returns the command's sequence number.
    pub fn enqueue(&mut self, head: [u8; COMMAND_BYTES], flags: u64) -> u64 {
        self.sequence += 1;
        let sequence = self.sequence;
        self.commands.push(Command {
            sequence,
            head,
            flags,
        });
        if self.results.len() == RESULT_LIMIT {
            self.results.pop_front();
            self.dropped += 1;
        }
        self.results.push_back(CommandResult {
            sequence,
            status: 0,
        });
        sequence
    }

    pub fn dequeue(&mut self) -> Option<CommandResult> {
        self.results.pop_front()
    }

    pub fn result_count(&self) -> usize {
        self.results.len()
    }

    /// The commands taken since the last call.
    pub fn take_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.commands)
    }

    pub fn dropped_results(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_completes_in_order() {
        let mut q = CommandQueue::default();
        assert!(!q.connected());
        let h = q.connect();
        assert!(h > 0 && h < 0x8000_0000);
        assert_eq!(q.connect(), h);
        let mut head = [0u8; COMMAND_BYTES];
        head[0] = 7;
        assert_eq!(q.enqueue(head, 0), 1);
        assert_eq!(q.enqueue([0; COMMAND_BYTES], 0), 2);
        assert_eq!(q.result_count(), 2);
        assert_eq!(
            q.dequeue(),
            Some(CommandResult {
                sequence: 1,
                status: 0
            })
        );
        assert_eq!(q.dequeue().map(|r| r.sequence), Some(2));
        assert_eq!(q.dequeue(), None);
        let commands = q.take_commands();
        assert_eq!(commands[0].first_word(), 7);
        assert!(q.take_commands().is_empty());
    }

    #[test]
    fn results_are_bounded_and_disconnect_clears() {
        let mut q = CommandQueue::default();
        q.connect();
        for _ in 0..RESULT_LIMIT + 5 {
            q.enqueue([0; COMMAND_BYTES], 0);
        }
        assert_eq!(q.result_count(), RESULT_LIMIT);
        assert_eq!(q.dropped_results(), 5);
        q.disconnect();
        assert_eq!(q.result_count(), 0);
        assert!(!q.connected());
    }
}
