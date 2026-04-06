use std::time::Instant;

#[derive(Debug)]
pub(crate) struct TimingWheel {
    buckets: Vec<Vec<u16>>,
    bucket_count: usize,
    resolution_ms: u64,
    current_bucket: usize,
    last_advance: Instant,
}

impl TimingWheel {
    pub(crate) fn new(bucket_count: usize, resolution_ms: u64) -> Self {
        let bucket_count = bucket_count.max(1);
        Self {
            buckets: (0..bucket_count).map(|_| Vec::new()).collect(),
            bucket_count,
            resolution_ms: resolution_ms.max(1),
            current_bucket: 0,
            last_advance: Instant::now(),
        }
    }

    pub(crate) fn insert(&mut self, id: u16, delay_ms: u64) {
        let steps = delay_ms.max(1).div_ceil(self.resolution_ms) as usize;
        let bucket = (self.current_bucket + steps.min(self.bucket_count - 1)) % self.bucket_count;
        self.buckets[bucket].push(id);
    }

    pub(crate) fn advance_into(&mut self, out: &mut Vec<u16>) {
        let now = Instant::now();
        let elapsed_ms = now.duration_since(self.last_advance).as_millis() as u64;
        let steps = (elapsed_ms / self.resolution_ms).min(self.bucket_count as u64) as usize;
        if steps == 0 {
            return;
        }

        for _ in 0..steps {
            self.current_bucket = (self.current_bucket + 1) % self.bucket_count;
            out.append(&mut self.buckets[self.current_bucket]);
        }

        self.last_advance = now;
    }
}
