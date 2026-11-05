//! GPU time per stage of the frame, from timestamp queries: the GPU writes
//! a clock value between stages, and the differences are how long each
//! stage took. The values come back a frame or two later (reading them
//! never waits for the GPU); while a readback is in flight, frames are
//! simply not measured.
//!
//! Needs timestamp queries inside command encoders; on adapters without
//! them the timer is off and the HUD shows no GPU times.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// Stages measured, in frame order.
pub const STAGES: [&str; 4] = ["SHADOWS", "FOG", "SCENE", "POST"];
const MARKS: u32 = STAGES.len() as u32 + 1;
const BYTES: u64 = MARKS as u64 * 8;

/// Readback states.
const IDLE: u8 = 0;
const COPIED: u8 = 1;
const MAPPING: u8 = 2;
const MAPPED: u8 = 3;

pub struct GpuTimer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    state: Arc<AtomicU8>,
    /// Nanoseconds per timestamp tick.
    period: f32,
    /// Smoothed milliseconds per stage.
    pub stages: [f32; STAGES.len()],
}

impl GpuTimer {
    /// Features to request for timing, if the adapter has them.
    pub fn features(adapter: &wgpu::Adapter) -> wgpu::Features {
        let wanted =
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        if adapter.features().contains(wanted) {
            wanted
        } else {
            wgpu::Features::empty()
        }
    }

    /// `None` if the device can't time.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        if !device
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS)
        {
            return None;
        }
        let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("gpu timer"),
            ty: wgpu::QueryType::Timestamp,
            count: MARKS,
        });
        let buffer = |label, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: BYTES,
                usage,
                mapped_at_creation: false,
            })
        };
        Some(Self {
            queries,
            resolve: buffer(
                "gpu timer resolve",
                wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            ),
            readback: buffer(
                "gpu timer readback",
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            ),
            state: Arc::new(AtomicU8::new(IDLE)),
            period: queue.get_timestamp_period(),
            stages: [0.0; STAGES.len()],
        })
    }

    /// Whether this frame is measured (the last readback is done).
    pub fn measuring(&self) -> bool {
        self.state.load(Ordering::Acquire) == IDLE
    }

    /// Marks the boundary before stage `i` (`STAGES.len()` = the end).
    pub fn mark(&self, encoder: &mut wgpu::CommandEncoder, i: u32) {
        if self.measuring() {
            encoder.write_timestamp(&self.queries, i);
        }
    }

    /// After the last mark: copies the results out for reading.
    pub fn resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        if !self.measuring() {
            return;
        }
        encoder.resolve_query_set(&self.queries, 0..MARKS, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.readback, 0, BYTES);
        self.state.store(COPIED, Ordering::Release);
    }

    /// After submitting: starts reading the copied results, and takes in
    /// results that have arrived.
    pub fn collect(&mut self, device: &wgpu::Device) {
        match self.state.load(Ordering::Acquire) {
            COPIED => {
                self.state.store(MAPPING, Ordering::Release);
                let state = self.state.clone();
                self.readback
                    .map_async(wgpu::MapMode::Read, .., move |result| {
                        state.store(
                            if result.is_ok() { MAPPED } else { IDLE },
                            Ordering::Release,
                        );
                    });
            }
            MAPPED => {
                if let Ok(view) = self.readback.get_mapped_range(..) {
                    let ticks: Vec<u64> = view
                        .chunks_exact(8)
                        .map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
                        .collect();
                    drop(view);
                    for (i, stage) in self.stages.iter_mut().enumerate() {
                        let ms = ticks[i + 1].saturating_sub(ticks[i]) as f32 * self.period / 1e6;
                        // Smooth, so the numbers are readable.
                        *stage += (ms.min(1000.0) - *stage) * 0.1;
                    }
                }
                self.readback.unmap();
                self.state.store(IDLE, Ordering::Release);
            }
            _ => {}
        }
        let _ = device.poll(wgpu::PollType::Poll);
    }
}
