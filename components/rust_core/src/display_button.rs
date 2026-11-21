//! A button of the display board (port of `main/displays/button.cpp`): a
//! small state machine over the pin level (low = pressed), polled by the
//! display task. Kept apart from the GPIO to be testable on the host.

pub const SHORTPRESS: u32 = 1 << 1;
pub const LONGPRESS: u32 = 1 << 2;
pub const FASTPRESS: u32 = 1 << 3;
pub const TIMEOUT: u32 = 1 << 4;
/// held down right now
pub const PRESSED: u32 = 1 << 5;

#[derive(Default)]
pub struct Button {
    longpress_ms: i64,
    state: u8,
    timeout_tmr: i64,
    timeout: i64,
    button_tmr: i64,
    lastpress: i64,
    event: u32,
    repctr: u32,
}

impl Button {
    pub fn new(longpress_ms: i64) -> Button {
        Button {
            longpress_ms,
            ..Default::default()
        }
    }

    /// One poll: `high` the pin level (released), `now_ms` the time.
    pub fn update(&mut self, high: bool, now_ms: i64) {
        match self.state {
            // wait for the release after start (or a long press)
            0 => {
                if high {
                    self.state = 1;
                }
            }
            1 => {
                self.event &= !PRESSED;
                if !high {
                    self.state = 2;
                    self.button_tmr = now_ms;
                }
                if self.timeout_tmr != 0
                    && self.timeout != 0
                    && now_ms > self.timeout_tmr + self.timeout
                {
                    self.event |= TIMEOUT;
                    self.timeout_tmr = 0;
                    self.timeout = 0;
                    self.state = 0;
                }
            }
            // pressed: long press, or the release time
            _ => {
                self.event |= PRESSED;
                if self.button_tmr != 0 && now_ms - self.button_tmr > self.longpress_ms {
                    self.event |= LONGPRESS;
                    self.state = 0;
                    self.button_tmr = 0;
                } else if high {
                    self.event |= SHORTPRESS;
                    if now_ms < self.lastpress + 1000 {
                        self.repctr += 1;
                        if self.repctr >= 5 {
                            self.lastpress = 0;
                            self.repctr = 0;
                            self.event |= FASTPRESS;
                        }
                    } else {
                        self.repctr = 0;
                    }
                    self.lastpress = now_ms;
                    self.state = 1;
                }
            }
        }
    }

    pub fn event(&self) -> u32 {
        self.event
    }

    pub fn clear(&mut self) {
        self.event = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_and_long_press() {
        let mut b = Button::new(5000);
        b.update(true, 1000);
        b.update(false, 1010);
        b.update(false, 1020);
        assert_eq!(b.event(), PRESSED);
        b.update(true, 1100);
        assert_eq!(b.event() & SHORTPRESS, SHORTPRESS);
        b.clear();
        b.update(true, 1200);
        assert_eq!(b.event(), 0);

        // held longer than the long press time
        b.update(false, 2000);
        b.update(false, 2010);
        b.update(false, 7100);
        assert_eq!(b.event() & LONGPRESS, LONGPRESS);
        assert_eq!(b.event() & SHORTPRESS, 0);
        b.clear();
        // the release after a long press is no short press
        b.update(true, 7200);
        b.update(true, 7300);
        assert_eq!(b.event(), 0);
    }

    #[test]
    fn held_at_start_is_no_press() {
        let mut b = Button::new(5000);
        b.update(false, 0);
        b.update(false, 100);
        assert_eq!(b.event(), 0);
    }
}
