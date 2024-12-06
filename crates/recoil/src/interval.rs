use rand::Rng;
use std::{thread, time::Duration};

#[derive(Debug, Clone)]
pub struct Interval {
    pub duration: Duration,
    pub multiplier: f64,
    pub max_duration: Option<Duration>,
    pub jitter: Option<(f64, f64)>,
}

impl Interval {
    pub(crate) fn sleep(&mut self, mut cancel: impl FnMut() -> bool) {
        let mul = match self.jitter {
            None => self.multiplier,
            Some((min, max)) => {
                let mut rng = rand::thread_rng();
                self.multiplier * rng.gen_range(min..=max)
            }
        };
        let duration = self.duration.mul_f64(mul).as_millis() / 100;
        for _ in 0..duration {
            if cancel() {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        self.duration = self.next();
    }

    pub(crate) fn next(&self) -> Duration {
        match self.max_duration {
            Some(max_duration) => {
                if self.duration >= max_duration
                    || self.duration.mul_f64(self.multiplier) >= max_duration
                {
                    max_duration
                } else {
                    self.duration.mul_f64(self.multiplier)
                }
            }
            None => self.duration.mul_f64(self.multiplier),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_interval() {
        let mut interval = Interval {
            duration: Duration::from_millis(100),
            multiplier: 2.0,
            max_duration: None,
            jitter: None,
        };
        interval.duration = interval.next();
        assert_eq!(interval.duration, Duration::from_millis(200));
        interval.duration = interval.next();
        assert_eq!(interval.duration, Duration::from_millis(400));
    }

    #[test]
    fn with_max_duration() {
        let mut interval = Interval {
            duration: Duration::from_millis(100),
            multiplier: 2.0,
            max_duration: Some(Duration::from_millis(300)),
            jitter: None,
        };
        interval.duration = interval.next();
        assert_eq!(interval.duration, Duration::from_millis(200));
        interval.duration = interval.next();
        assert_eq!(interval.duration, Duration::from_millis(300));
        interval.duration = interval.next();
        assert_eq!(interval.duration, Duration::from_millis(300));
    }

    #[test]
    fn with_jitter() {
        let mut interval = Interval {
            duration: Duration::from_millis(100),
            multiplier: 2.0,
            max_duration: None,
            jitter: Some((0.9, 1.1)),
        };

        interval.duration = interval.next();
        assert!(interval.duration >= Duration::from_millis(180));
        assert!(interval.duration <= Duration::from_millis(220));

        interval.duration = interval.next();
        assert!(interval.duration >= Duration::from_millis(360));
        assert!(interval.duration <= Duration::from_millis(440));
    }

    #[test]
    fn with_max_duration_and_jitter() {
        let mut interval = Interval {
            duration: Duration::from_millis(100),
            multiplier: 2.0,
            max_duration: Some(Duration::from_millis(300)),
            jitter: Some((0.9, 1.1)),
        };

        interval.duration = interval.next();
        assert!(interval.duration >= Duration::from_millis(180));
        assert!(interval.duration <= Duration::from_millis(220));

        interval.duration = interval.next();
        assert!(interval.duration >= Duration::from_millis(270));
        assert!(interval.duration <= Duration::from_millis(330));

        interval.duration = interval.next();
        assert!(interval.duration >= Duration::from_millis(270));
        assert!(interval.duration <= Duration::from_millis(330));
    }
}
