use std::collections::VecDeque;

pub struct RingBuffer {
    buffer: VecDeque<i16>,
    capacity: usize,
}

impl RingBuffer {
    pub fn new(sample_rate: u32, duration_secs: f32) -> Self {
        let capacity = (sample_rate as f32 * duration_secs) as usize;
        Self {
            buffer: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, samples: &[i16]) {
        for &sample in samples {
            if self.buffer.len() >= self.capacity {
                self.buffer.pop_front();
            }
            self.buffer.push_back(sample);
        }
    }

    pub fn drain(&mut self) -> Vec<i16> {
        self.buffer.drain(..).collect()
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_buffer() {
        let mut buf = RingBuffer::new(16000, 5.0); // 5 seconds at 16kHz
        assert_eq!(buf.len(), 0);
        assert!(buf.drain().is_empty());
    }

    #[test]
    fn test_push_and_drain() {
        let mut buf = RingBuffer::new(16000, 5.0);
        let samples: Vec<i16> = (0..1000).collect();
        buf.push(&samples);
        assert_eq!(buf.len(), 1000);
        let drained = buf.drain();
        assert_eq!(drained.len(), 1000);
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn test_overflow_evicts_oldest() {
        let mut buf = RingBuffer::new(16000, 1.0); // 1 second = 16000 samples
        let samples: Vec<i16> = (0..20000).map(|i| i as i16).collect();
        buf.push(&samples);
        assert_eq!(buf.len(), 16000); // capped at capacity
        let drained = buf.drain();
        // Should contain the newest 16000 samples (4000..20000)
        assert_eq!(drained[0], 4000);
    }
}
