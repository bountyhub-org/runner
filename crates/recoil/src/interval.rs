use rand::Rng;
use std::{thread, time::Duration};

#[derive(Debug, Clone, Copy)]
pub struct Interval {
    pub initial_duration: Duration,
    pub multiplier: f64,
    pub max_duration: Option<Duration>,
    pub jitter: Option<(f64, f64)>,
}

impl Interval {
    pub(crate) fn sleep(&mut self, mut cancel: impl FnMut() -> bool) {
        let mul = match self.jitter {
            None => self.multiplier,
            Some((min, max)) => {
                let mut rng = rand::rng();
                self.multiplier * rng.random_range(min..=max)
            }
        };
        let duration = self.initial_duration.mul_f64(mul).as_millis() / 100;
        for _ in 0..duration {
            if cancel() {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        self.initial_duration = self.next();
    }

    pub(crate) fn next(&self) -> Duration {
        match self.max_duration {
            Some(max_duration) => {
                if self.initial_duration >= max_duration
                    || self.initial_duration.mul_f64(self.multiplier) >= max_duration
                {
                    max_duration
                } else {
                    self.initial_duration.mul_f64(self.multiplier)
                }
            }
            None => self.initial_duration.mul_f64(self.multiplier),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_interval() {
        let mut interval = Interval {
            initial_duration: Duration::from_millis(100),
            multiplier: 2.0,
            max_duration: None,
            jitter: None,
        };
        interval.initial_duration = interval.next();
        assert_eq!(interval.initial_duration, Duration::from_millis(200));
        interval.initial_duration = interval.next();
        assert_eq!(interval.initial_duration, Duration::from_millis(400));
    }

    #[test]
    fn with_max_duration() {
        let mut interval = Interval {
            initial_duration: Duration::from_millis(100),
            multiplier: 2.0,
            max_duration: Some(Duration::from_millis(300)),
            jitter: None,
        };
        interval.initial_duration = interval.next();
        assert_eq!(interval.initial_duration, Duration::from_millis(200));
        interval.initial_duration = interval.next();
        assert_eq!(interval.initial_duration, Duration::from_millis(300));
        interval.initial_duration = interval.next();
        assert_eq!(interval.initial_duration, Duration::from_millis(300));
    }

    #[test]
    fn with_jitter() {
        let mut interval = Interval {
            initial_duration: Duration::from_millis(100),
            multiplier: 2.0,
            max_duration: None,
            jitter: Some((0.9, 1.1)),
        };

        interval.initial_duration = interval.next();
        assert!(interval.initial_duration >= Duration::from_millis(180));
        assert!(interval.initial_duration <= Duration::from_millis(220));

        interval.initial_duration = interval.next();
        assert!(interval.initial_duration >= Duration::from_millis(360));
        assert!(interval.initial_duration <= Duration::from_millis(440));
    }

    #[test]
    fn with_max_duration_and_jitter() {
        let mut interval = Interval {
            initial_duration: Duration::from_millis(100),
            multiplier: 2.0,
            max_duration: Some(Duration::from_millis(300)),
            jitter: Some((0.9, 1.1)),
        };

        interval.initial_duration = interval.next();
        assert!(interval.initial_duration >= Duration::from_millis(180));
        assert!(interval.initial_duration <= Duration::from_millis(220));

        interval.initial_duration = interval.next();
        assert!(interval.initial_duration >= Duration::from_millis(270));
        assert!(interval.initial_duration <= Duration::from_millis(330));

        interval.initial_duration = interval.next();
        assert!(interval.initial_duration >= Duration::from_millis(270));
        assert!(interval.initial_duration <= Duration::from_millis(330));
    }
}
