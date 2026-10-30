//! Asynchronous GPU-to-CPU frame capture, for outputs that need pixels in
//! memory (NDI, CPU fallbacks).
//!
//! Each frame copies the output texture into one of a few mappable buffers;
//! a buffer is read back once the GPU is done with it, a frame or two later.
//! Rendering never waits for a readback.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

const SLOTS: usize = 3;

const FREE: u8 = 0;
/// Copy recorded; waiting for submit to request the mapping.
const COPIED: u8 = 1;
const MAPPING: u8 = 2;
const READY: u8 = 3;
const FAILED: u8 = 4;

struct Slot {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    /// Capture order, to deliver the newest ready frame.
    sequence: u64,
}

/// A captured RGBA8 frame, rows top to bottom without padding.
pub struct GrabbedFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

pub struct FrameGrabber {
    size: (u32, u32),
    row_bytes: u32,
    slots: Vec<Slot>,
    sequence: u64,
}

impl FrameGrabber {
    pub fn new(device: &wgpu::Device, size: (u32, u32)) -> Self {
        let row_bytes = (size.0 * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let slots = (0..SLOTS)
            .map(|_| Slot {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("frame grab"),
                    size: (row_bytes * size.1) as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(FREE)),
                sequence: 0,
            })
            .collect();
        Self {
            size,
            row_bytes,
            slots,
            sequence: 0,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Records a copy of `texture` (RGBA8, same size) when a buffer is free.
    /// Returns `false` when all buffers are still busy and the frame is skipped.
    pub fn capture(&mut self, encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture) -> bool {
        let Some(slot) = self
            .slots
            .iter_mut()
            .find(|s| s.state.load(Ordering::Acquire) == FREE)
        else {
            return false;
        };
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &slot.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.row_bytes),
                    rows_per_image: Some(self.size.1),
                },
            },
            wgpu::Extent3d {
                width: self.size.0,
                height: self.size.1,
                depth_or_array_layers: 1,
            },
        );
        self.sequence += 1;
        slot.sequence = self.sequence;
        slot.state.store(COPIED, Ordering::Release);
        true
    }

    /// Requests mappings for copies recorded before the last submit.
    pub fn after_submit(&mut self) {
        for slot in &self.slots {
            if slot.state.load(Ordering::Acquire) != COPIED {
                continue;
            }
            slot.state.store(MAPPING, Ordering::Release);
            let state = slot.state.clone();
            slot.buffer
                .map_async(wgpu::MapMode::Read, .., move |result| {
                    state.store(
                        if result.is_ok() { READY } else { FAILED },
                        Ordering::Release,
                    );
                });
        }
    }

    /// Returns the newest finished frame, if any, without waiting for the GPU.
    /// Older finished frames are dropped.
    pub fn poll(&mut self, device: &wgpu::Device) -> Option<GrabbedFrame> {
        let _ = device.poll(wgpu::PollType::Poll);
        let ready: Vec<usize> = (0..self.slots.len())
            .filter(|&i| self.slots[i].state.load(Ordering::Acquire) == READY)
            .collect();
        let newest = ready
            .iter()
            .copied()
            .max_by_key(|&i| self.slots[i].sequence);
        let mut frame = None;
        for i in ready {
            let slot = &self.slots[i];
            if Some(i) == newest {
                let mapped = slot.buffer.get_mapped_range(..).ok()?;
                let row = (self.size.0 * 4) as usize;
                let mut data = Vec::with_capacity(row * self.size.1 as usize);
                for line in mapped.chunks(self.row_bytes as usize) {
                    data.extend_from_slice(&line[..row]);
                }
                drop(mapped);
                frame = Some(GrabbedFrame {
                    width: self.size.0,
                    height: self.size.1,
                    data,
                });
            }
            slot.buffer.unmap();
            slot.state.store(FREE, Ordering::Release);
        }
        for slot in &self.slots {
            if slot.state.load(Ordering::Acquire) == FAILED {
                slot.state.store(FREE, Ordering::Release);
            }
        }
        frame
    }
}
