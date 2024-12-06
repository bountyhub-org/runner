use crate::interval::Interval;

pub enum State<T, C, E>
where
    C: FnMut() -> bool,
    T: Send,
    E: Send,
{
    Done(T),
    Retry(C),
    Fail(E),
}

#[derive(Debug, PartialEq)]
pub enum Error<C>
where
    C: Send,
{
    MaxRetriesReached,
    Custom(C),
}

#[derive(Clone, Debug)]
pub struct Recoil {
    pub interval: Interval,
    pub max_retries: Option<usize>,
}

impl Recoil {
    pub fn run<F, C, T, E>(&mut self, mut f: F) -> Result<T, Error<E>>
    where
        F: FnMut() -> State<T, C, E>,
        T: Send,
        E: Send,
        C: FnMut() -> bool + Send,
    {
        let mut retries = 0;
        loop {
            if let Some(max_retries) = self.max_retries {
                if retries > max_retries {
                    return Err(Error::MaxRetriesReached);
                }
            }
            match f() {
                State::Done(t) => return Ok(t),
                State::Retry(cancel) => {
                    retries += 1;
                    self.interval.sleep(cancel)
                }
                State::Fail(e) => return Err(Error::Custom(e)),
            }
        }
    }

    pub fn restart(&mut self, interval: Interval) {
        self.interval = interval;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interval::Interval;
    use std::{cell::RefCell, time::Duration};

    #[test]
    fn test_backoff() {
        let interval = Interval {
            duration: Duration::from_millis(100),
            multiplier: 2.0,
            max_duration: Some(Duration::from_secs(600)),
            jitter: Some((0.9, 1.1)),
        };

        let runs = RefCell::new(0);
        let fail = RefCell::new(false);
        let func = || {
            *runs.borrow_mut() += 1;
            if *runs.borrow() == 3 {
                if *fail.borrow() {
                    State::Fail("fail".to_string())
                } else {
                    State::Done(())
                }
            } else {
                State::Retry(Box::new(|| false))
            }
        };

        let mut backoff = Recoil {
            interval: interval.clone(),
            max_retries: Some(2),
        };

        assert_eq!(backoff.run(func), Ok(()));

        backoff.restart(interval);
        *fail.borrow_mut() = true;
        *runs.borrow_mut() = 0;

        assert_eq!(backoff.run(func), Err(Error::Custom("fail".to_string())));
    }

    #[test]
    fn test_max_retries() {
        let interval = Interval {
            duration: Duration::from_millis(100),
            multiplier: 2.0,
            max_duration: Some(Duration::from_secs(600)),
            jitter: Some((0.9, 1.1)),
        };

        let runs = RefCell::new(0);
        let retry = || true;

        let mut recoil = Recoil {
            interval: interval.clone(),
            max_retries: Some(2),
        };

        let res = recoil.run(|| {
            *runs.borrow_mut() += 1;
            if *runs.borrow() == 4 {
                State::Done(())
            } else if *runs.borrow() == 5 {
                State::Fail("fail".to_string())
            } else {
                State::Retry(Box::new(retry))
            }
        });

        assert_eq!(res, Err(Error::MaxRetriesReached));
    }
}
