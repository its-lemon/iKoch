//! Shared monotonic clock and timestamp conversion helpers for A/V sync.

use std::time::Instant;

/// Media Foundation 100-nanosecond ticks per microsecond.
pub const HNS_PER_MICROSECOND: i64 = 10;

/// Standard MP4 video timescale (90 kHz).
pub const VIDEO_TIMESCALE_90KHZ: u32 = 90_000;

/// Standard MP4 audio timescale matching 48 kHz PCM/AAC.
pub const AUDIO_TIMESCALE_48KHZ: u32 = 48_000;

/// Single monotonic timebase shared across WGC video capture and WASAPI audio streams.
#[derive(Debug, Clone, Copy)]
pub struct MonotonicClock {
    origin: Instant,
}

impl MonotonicClock {
    /// Creates a new monotonic clock anchored at the current instant.
    #[must_use]
    pub fn now() -> Self {
        Self {
            origin: Instant::now(),
        }
    }

    /// Creates a monotonic clock anchored at a specific `Instant`.
    #[must_use]
    pub const fn from_origin(origin: Instant) -> Self {
        Self { origin }
    }

    /// Returns the underlying origin `Instant`.
    #[must_use]
    pub const fn origin(&self) -> Instant {
        self.origin
    }

    /// Returns elapsed microseconds from the clock origin.
    #[must_use]
    pub fn now_us(&self) -> i64 {
        self.instant_to_us(Instant::now())
    }

    /// Converts an arbitrary `Instant` into signed microseconds relative to `origin`.
    #[must_use]
    pub fn instant_to_us(&self, instant: Instant) -> i64 {
        if instant >= self.origin {
            let delta = instant.duration_since(self.origin).as_micros();
            i64::try_from(delta).unwrap_or(i64::MAX)
        } else {
            let delta = self.origin.duration_since(instant).as_micros();
            let signed = i64::try_from(delta).unwrap_or(i64::MAX);
            signed.saturating_neg()
        }
    }
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::now()
    }
}

/// Converts microseconds (`us`) into Windows Media Foundation 100-nanosecond (`HNS`) units.
#[must_use]
pub const fn us_to_hns(us: i64) -> i64 {
    us.saturating_mul(HNS_PER_MICROSECOND)
}

/// Converts Windows Media Foundation 100-nanosecond (`HNS`) units into microseconds (`us`).
#[must_use]
pub const fn hns_to_us(hns: i64) -> i64 {
    hns / HNS_PER_MICROSECOND
}

/// Converts signed microseconds into an arbitrary MP4 media timescale (e.g., 90 kHz or 48 kHz).
#[must_use]
pub const fn us_to_timescale(us: i64, timescale: u32) -> i64 {
    if timescale == 0 {
        return 0;
    }
    let scaled = (us as i128) * (timescale as i128) / 1_000_000_i128;
    if scaled > i64::MAX as i128 {
        i64::MAX
    } else if scaled < i64::MIN as i128 {
        i64::MIN
    } else {
        scaled as i64
    }
}

/// Converts ticks in a given MP4 timescale back to signed microseconds.
#[must_use]
pub const fn timescale_to_us(ticks: i64, timescale: u32) -> i64 {
    if timescale == 0 {
        return 0;
    }
    let scaled = (ticks as i128) * 1_000_000_i128 / (timescale as i128);
    if scaled > i64::MAX as i128 {
        i64::MAX
    } else if scaled < i64::MIN as i128 {
        i64::MIN
    } else {
        scaled as i64
    }
}
