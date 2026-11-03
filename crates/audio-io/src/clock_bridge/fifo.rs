//! Worker-owned frame FIFO over storage prepared before publication.
pub(super) struct FrameFifo {
    samples: Vec<f32>,
    channels: usize,
    read: usize,
    frames: usize,
}

impl FrameFifo {
    pub(super) fn new(samples: Vec<f32>, channels: usize) -> Self {
        Self {
            samples,
            channels,
            read: 0,
            frames: 0,
        }
    }
    pub(super) fn frames(&self) -> usize {
        self.frames
    }
    pub(super) fn available(&self) -> usize {
        self.samples.len() / self.channels - self.frames
    }
    pub(super) fn clear(&mut self) {
        self.read = 0;
        self.frames = 0;
    }
    pub(super) fn push(&mut self, samples: &[f32]) {
        let start = (self.read + self.frames * self.channels) % self.samples.len();
        let first = samples.len().min(self.samples.len() - start);
        self.samples[start..start + first].copy_from_slice(&samples[..first]);
        self.samples[..samples.len() - first].copy_from_slice(&samples[first..]);
        self.frames += samples.len() / self.channels;
    }
    pub(super) fn peek(&self, samples: &mut [f32]) {
        let first = samples.len().min(self.samples.len() - self.read);
        samples[..first].copy_from_slice(&self.samples[self.read..self.read + first]);
        let rest = samples.len() - first;
        samples[first..].copy_from_slice(&self.samples[..rest]);
    }
    pub(super) fn consume(&mut self, frames: usize) {
        self.read = (self.read + frames * self.channels) % self.samples.len();
        self.frames -= frames;
    }
}
