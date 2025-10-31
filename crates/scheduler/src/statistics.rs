pub trait RuntimeStatistic: Send + Sync + Default {
    fn update(&mut self, time: u64);
    fn value(&self) -> u64;
}

#[derive(Debug)]
pub struct RunningMean {
    // statistics are stored as ms in a u64
    running_mean: u64,
    samples: u64,
}

impl RunningMean {
    fn new() -> Self {
        RunningMean {
            running_mean: u64::MAX,
            samples: 0,
        }
    }
}

impl Default for RunningMean {
    fn default() -> Self {
        RunningMean::new()
    }
}

impl RuntimeStatistic for RunningMean {
    fn update(&mut self, time: u64) {
        if self.samples == 0 {
            self.running_mean = time;
            self.samples = 1;
        } else {
            self.samples += 1;
            self.running_mean = (self.running_mean as i64
                + (time as i64 - self.running_mean as i64) / self.samples as i64)
                as u64;
        }
    }

    fn value(&self) -> u64 {
        self.running_mean
    }
}

#[cfg(test)]
mod tests {
    use crate::statistics::{RunningMean, RuntimeStatistic};

    #[test]
    fn running_mean_update() {
        let mut running_mean = RunningMean::new();

        running_mean.update(1050);
        assert_eq!(running_mean.samples, 1, "First update");
        assert_eq!(running_mean.value(), 1050);

        running_mean.update(1000);
        assert_eq!(running_mean.samples, 2, "Second update");
        assert_eq!(running_mean.value(), 1025);

        running_mean.update(1025);
        assert_eq!(running_mean.samples, 3, "Third update");
        assert_eq!(running_mean.value(), 1025);

        running_mean.update(2050);
        assert_eq!(running_mean.samples, 4, "Fourth update");
        assert_eq!(running_mean.value(), 1281);
    }
}
