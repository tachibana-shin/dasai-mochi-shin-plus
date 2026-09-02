//! TTP233 capacitive-touch sensor (single button) + gesture recognition.
//!
//! The board has exactly one touch input (`GPIO10`), so the firmware leans on
//! a handful of gestures built from it: single / double / triple tap and a
//! long press. [`TapInput`] owns the pinned [`Input`], debounces the signal
//! and turns a stream of debounced edges into a [`Gesture`].
//!
//! Because the firmware is a blocking loop, the caller must poll
//! [`TapInput::poll`] frequently (every screen loop / small delay) with the
//! current [`Instant`]; it returns `Some` only once per completed gesture. A
//! long press fires [`Gesture::Press`] once as soon as the hold threshold is
//! crossed — [`TapInput::is_held`] lets callers implement auto-repeat while
//! the sensor stays touched.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;

use esp_hal::gpio::{Input, InputConfig, InputPin, Pull};
use esp_hal::time::{Duration, Instant};

use crate::config;

/// A completed gesture recognised from the tap sensor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    /// One quick touch-and-release.
    Tap,
    /// Two quick touches in the multi-tap window.
    DoubleTap,
    /// Three quick touches in the multi-tap window.
    TripleTap,
    /// A touch held past [`config::TAP.press_ms`].
    Press,
}

/// Debounced, gesture-recognising touch input for the TTP233 sensor.
pub struct TapInput {
    input: Input<'static>,
    /// Raw level read from the pin on the last poll (before debounce).
    raw_level: bool,
    /// Debounced `touch` boolean (true = finger is on the sensor).
    touched: bool,
    /// Debounced level from the previous poll (for edge detection).
    prev_touch: bool,
    /// Set while a raw change has not yet been confirmed by the debounce timer.
    changing: bool,
    /// Instant the raw level last changed (start of the debounce window).
    change_at: Instant,
    /// When the current touch began (for measuring a long press).
    touch_start: Instant,
    /// Number of taps accumulated in the current multi-tap window.
    taps: u32,
    /// True once a `Press` has already fired for the current touch.
    press_fired: bool,
    /// Instant of the most recent release (closes the multi-tap window).
    last_release: Instant,
}

impl TapInput {
    /// Attach to the sensor pin. Uses [`config::TAP.active_high`] (TTP233
    /// default: a touch reads HIGH) and pulls the pin down so a floating line
    /// reads as "not touched".
    pub fn new(pin: impl InputPin + 'static) -> Self {
        let input = Input::new(pin, InputConfig::default().with_pull(Pull::Down));
        Self {
            input,
            raw_level: false,
            touched: false,
            prev_touch: false,
            changing: false,
            change_at: Instant::EPOCH,
            touch_start: Instant::EPOCH,
            taps: 0,
            press_fired: false,
            last_release: Instant::EPOCH,
        }
    }

    /// The raw (un-debounced) pin level as a `touch` boolean.
    fn raw_touch(&self) -> bool {
        if config::TAP.active_high {
            self.input.is_high()
        } else {
            self.input.is_low()
        }
    }

    /// True while the sensor is being touched (post-debounce).
    pub fn is_held(&self) -> bool {
        self.touched
    }

    /// Advance the state machine with the given `now`, returning a completed
    /// gesture as soon as one is detected (at most one per tick).
    pub fn poll(&mut self, now: Instant) -> Option<Gesture> {
        let raw = self.raw_touch();
        self.raw_level = raw;
        self.debounce(now, raw);

        let touch = self.touched;
        let rising = touch && !self.prev_touch;
        let falling = !touch && self.prev_touch;
        self.prev_touch = touch;

        let multi_ms = Duration::from_millis(config::TAP.multitap_window_ms as u64);
        let press_ms = Duration::from_millis(config::TAP.press_ms as u64);

        // Rising edge: a new touch begins.
        if rising {
            self.touch_start = now;
            self.press_fired = false;
        }

        // Long press: fire once the hold threshold is crossed, but only once.
        if touch && !self.press_fired && now - self.touch_start >= press_ms {
            self.press_fired = true;
            return Some(Gesture::Press);
        }

        // Falling edge: a touch was released.
        if falling {
            if self.press_fired {
                // A long-press was released; don't count it as a tap.
                self.taps = 0;
            } else {
                // A short press ended -> one tap, keep counting the window.
                self.taps = (self.taps + 1).min(3);
            }
            self.press_fired = false;
            self.last_release = now;
            return None;
        }

        // While released: close the multi-tap window once it expires.
        if !touch && self.taps > 0 && now - self.last_release >= multi_ms {
            let gesture = match self.taps {
                1 => Gesture::Tap,
                2 => Gesture::DoubleTap,
                _ => Gesture::TripleTap,
            };
            self.taps = 0;
            return Some(gesture);
        }

        None
    }

    /// Debounce the raw level into [`TapInput::touched`] once it has been
    /// stable for [`config::TAP.debounce_ms`].
    fn debounce(&mut self, now: Instant, raw: bool) {
        let debounce_ms = Duration::from_millis(config::TAP.debounce_ms as u64);
        if raw != self.touched {
            if !self.changing {
                self.changing = true;
                self.change_at = now;
            } else if now - self.change_at >= debounce_ms {
                self.touched = raw;
                self.changing = false;
            }
        } else {
            self.changing = false;
        }
    }
}

// ---------------------------------------------------------------------------
// Gesture transport: light sensor task -> display task
// ---------------------------------------------------------------------------

/// Bounded channel that carries recognised [`Gesture`]s from the lightweight
/// sensor task to the (heavier) display/router task.
///
/// This is the Rust analogue of the C++ firmware's two-task design: one task
/// keeps polling the tap sensor and publishing completed gestures here, while
/// the display task consumes them without ever touching the GPIO directly.
pub type TapChannel = Channel<CriticalSectionRawMutex, Gesture, 8>;

/// Try to receive a completed gesture without blocking. Returns `None` if the
/// channel is empty — used for poll-style checks inside render loops.
pub fn try_tap(ch: &'static TapChannel) -> Option<Gesture> {
    ch.receiver().try_receive().ok()
}

/// Wait (asynchronously) for the next completed gesture. The caller yields to
/// the executor until a gesture is available.
pub async fn wait_tap(ch: &'static TapChannel) -> Gesture {
    ch.receive().await
}
