//! Clock abstraction for testable time operations
//!
//! This module provides a simple abstraction over system time operations to enable
//! deterministic testing of time-dependent code. The pattern allows injecting either
//! a real system clock or a controllable mock clock.
//!
//! # Examples
//!
//! ```
//! use hypha_clock::{Clock, SystemClock};
//! use chrono::Utc;
//!
//! let clock = SystemClock;
//! let now = clock.now();
//! ```
//!
//! # Testing
//!
//! ```
//! #[cfg(test)]
//! use hypha_clock::MockClock;
//! use chrono::Duration;
//!
//! #[cfg(test)]
//! fn test_time_dependent_code() {
//!     let clock = MockClock::at_epoch();
//!     // Your time-dependent code here
//!     clock.advance(Duration::seconds(60));
//!     // Assert behavior after time has passed
//! }
//! ```

#[cfg(any(test, feature = "testing"))]
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};

/// Trait for abstracting time operations to enable testing
pub trait Clock: Send + Sync {
    /// Returns the current time
    fn now(&self) -> DateTime<Utc>;
}

/// System clock implementation that uses actual system time
#[derive(Debug, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Mock clock implementation for testing
///
/// This clock allows you to control time in tests by setting specific times
/// or advancing by durations. It's only available in test builds or when
/// the "testing" feature is enabled.
///
/// # Examples
///
/// ```
/// # #[cfg(any(test, feature = "testing"))]
/// # {
/// use hypha_clock::MockClock;
/// use chrono::Duration;
///
/// let clock = MockClock::at_current_time();
/// clock.advance(Duration::hours(2));
/// # }
/// ```
#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Clone)]
pub struct MockClock {
    time: Arc<Mutex<DateTime<Utc>>>,
}

#[cfg(any(test, feature = "testing"))]
impl MockClock {
    /// Create a new mock clock with a specific time
    pub fn new(time: DateTime<Utc>) -> Self {
        Self {
            time: Arc::new(Mutex::new(time)),
        }
    }

    /// Advance the clock by the given duration
    pub fn advance(&self, duration: chrono::Duration) {
        // NOTE: We handle lock poisoning gracefully here to avoid panics in tests
        let mut time = self.time.lock().unwrap_or_else(|e| e.into_inner());
        *time = *time + duration;
    }

    /// Set the clock to a specific time
    pub fn set(&self, new_time: DateTime<Utc>) {
        let mut time = self.time.lock().unwrap_or_else(|e| e.into_inner());
        *time = new_time;
    }
}

#[cfg(any(test, feature = "testing"))]
impl Default for MockClock {
    fn default() -> Self {
        Self::new(Utc::now())
    }
}

#[cfg(any(test, feature = "testing"))]
impl Clock for MockClock {
    fn now(&self) -> DateTime<Utc> {
        *self.time.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_clock() {
        let clock = SystemClock;
        // NOTE: We only verify that calling now() doesn't panic. Testing actual time progression is inherently flaky on loaded systems.
        let _ = clock.now();
    }

    #[test]
    fn test_mock_clock() {
        let start_time = Utc::now();
        let clock = MockClock::new(start_time);

        assert_eq!(clock.now(), start_time);

        let advance_duration = chrono::Duration::seconds(60);
        clock.advance(advance_duration);
        assert_eq!(clock.now(), start_time + advance_duration);

        let new_time = start_time + chrono::Duration::days(1);
        clock.set(new_time);
        assert_eq!(clock.now(), new_time);
    }

    #[test]
    fn test_system_clock_is_copy() {
        let clock1 = SystemClock;
        let clock2 = clock1; // This should work because SystemClock is Copy
        let _ = clock1.now(); // We can still use clock1
        let _ = clock2.now();
    }
}
