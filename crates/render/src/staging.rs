use std::collections::VecDeque;

use ash::vk;

use crate::buffer::{Buffer, BufferDesc, MemoryLocation};
use crate::device::{Queue, Submission};
use crate::error::{RenderError, Result};

/// A piece of a [`StagingRing`]'s buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StagingSlice {
    pub offset: u64,
    pub size: u64,
}

/// A persistently mapped ring of staging memory, for uploads (`MemoryLocation::Upload`) or readbacks
/// (`MemoryLocation::Readback`).
///
/// `alloc` hands out slices; after submitting the work that uses them, `mark_submitted` ties everything handed
/// out so far to that submission. A later `alloc` that needs room waits for the oldest submission to finish.
pub struct StagingRing {
    buffer: Buffer,
    capacity: u64,
    /// Total bytes handed out and total bytes known free, as counters that never wrap; `% capacity` gives offsets.
    head: u64,
    tail: u64,
    marked: u64,
    in_flight: VecDeque<(u64, Submission)>,
}

pub(crate) fn align_up(value: u64, align: u64) -> u64 {
    value.div_ceil(align) * align
}

impl StagingRing {
    pub fn new(queue: &Queue, capacity: u64, location: MemoryLocation) -> Result<StagingRing> {
        let buffer = Buffer::new(
            queue.device(),
            &BufferDesc {
                name: "staging ring",
                size: capacity,
                usage: vk::BufferUsageFlags::empty(),
                location,
            },
        )?;
        Ok(StagingRing {
            capacity: buffer.size(),
            buffer,
            head: 0,
            tail: 0,
            marked: 0,
            in_flight: VecDeque::new(),
        })
    }

    /// The ring's buffer, as the source (upload) or destination (readback) of copies.
    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    /// Reserves `size` bytes at `align`. Waits for older submissions when the ring is full; fails with
    /// `StagingFull` when only unsubmitted slices stand in the way, and with `Invalid` when `size` can never fit.
    pub fn alloc(&mut self, size: u64, align: u64) -> Result<StagingSlice> {
        let align = align.max(1);
        if size > self.capacity {
            return Err(RenderError::Invalid(format!(
                "a {size}-byte staging slice does not fit a {}-byte ring",
                self.capacity
            )));
        }
        loop {
            let mut start = align_up(self.head, align);
            if start % self.capacity + size > self.capacity {
                start = align_up(start, self.capacity);
            }
            if start + size - self.tail <= self.capacity {
                self.head = start + size;
                return Ok(StagingSlice {
                    offset: start % self.capacity,
                    size,
                });
            }
            let Some((end, submission)) = self.in_flight.pop_front() else {
                return Err(RenderError::StagingFull);
            };
            submission.wait()?;
            self.tail = end;
        }
    }

    /// Reserves a slice and copies `data` into it.
    pub fn upload(&mut self, data: &[u8], align: u64) -> Result<StagingSlice> {
        let slice = self.alloc(data.len() as u64, align)?;
        self.buffer.write(slice.offset, data)?;
        Ok(slice)
    }

    /// Copies a slice's bytes out of a readback ring; the copy into the ring must have completed.
    pub fn read(&self, slice: StagingSlice) -> Result<Vec<u8>> {
        self.buffer.read_vec(slice.offset, slice.size as usize)
    }

    /// Ties every slice handed out since the last call to `submission`.
    pub fn mark_submitted(&mut self, submission: &Submission) {
        if self.head > self.marked {
            self.marked = self.head;
            self.in_flight.push_back((self.head, submission.clone()));
        }
    }
}
