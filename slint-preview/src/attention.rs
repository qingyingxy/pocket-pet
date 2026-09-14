// Coarse monotonic scheduling: no background animation or frame-rate polling.
pub const FIRST_DELAY: u64 = 60 * 60;
#[derive(Debug)]
pub struct Attention {
    next_at: u64,
    interval: u64,
}
impl Attention {
    pub fn new(now: u64) -> Self {
        Self {
            next_at: now + FIRST_DELAY,
            interval: FIRST_DELAY,
        }
    }
    pub fn viewed(&mut self, now: u64) {
        *self = Self::new(now);
    }
    pub fn tick(&mut self, now: u64, pending: bool, viewing: bool, busy: bool) -> bool {
        if viewing || !pending {
            self.viewed(now);
            return false;
        }
        if busy || now < self.next_at {
            return false;
        }
        self.interval = (self.interval * 2).min(4 * FIRST_DELAY);
        self.next_at = now + self.interval;
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn waits_backs_off_and_resets_after_viewing() {
        let mut a = Attention::new(0);
        assert!(!a.tick(3599, true, false, false));
        assert!(a.tick(3600, true, false, false));
        assert!(!a.tick(10799, true, false, false));
        assert!(a.tick(10800, true, false, false));
        a.viewed(11000);
        assert!(!a.tick(14599, true, false, false));
        assert!(a.tick(14600, true, false, false));
    }
    #[test]
    fn stays_quiet_when_empty_visible_or_busy() {
        let mut a = Attention::new(0);
        assert!(!a.tick(3600, false, false, false));
        assert!(!a.tick(7200, true, true, false));
        assert!(!a.tick(10800, true, false, true));
        assert!(a.tick(10830, true, false, false));
    }
}
