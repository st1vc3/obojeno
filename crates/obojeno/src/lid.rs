//! Following the MacBook lid for the lid animation (`lid_view` draws it).
//!
//! The lid-angle sensor is "las" on the Apple SPU, HID usage page 0x20,
//! usage 0x8A. Its feature report 1 holds the angle in degrees as a
//! little-endian u16: 0 closed, about 100-130 in use. Reading it needs no
//! permission. Desktop Macs and older MacBooks have no sensor.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use objc2::MainThreadMarker;

use objc2_core_foundation::{CFDictionary, CFIndex, CFNumber, CFRetained, CFString};
use objc2_io_kit::{IOHIDDevice, IOHIDManager, IOHIDReportType, kIOReturnSuccess};

struct LidSensor {
    // The manager owns the matched device; keep it alive.
    _manager: CFRetained<IOHIDManager>,
    device: CFRetained<IOHIDDevice>,
}

// SAFETY: IOHIDDevice reports can be read from any thread; the sensor is
// only ever used by one thread at a time.
unsafe impl Send for LidSensor {}

impl LidSensor {
    /// The sensor, if this Mac has one.
    fn open() -> Option<Self> {
        let manager = IOHIDManager::new(None, 0);
        let key = CFString::from_static_str;
        let matching = CFDictionary::<CFString, CFNumber>::from_slices(
            &[
                &key("VendorID"),
                &key("ProductID"),
                &key("PrimaryUsagePage"),
                &key("PrimaryUsage"),
            ],
            &[
                &CFNumber::new_i32(0x05AC),
                &CFNumber::new_i32(0x8104),
                &CFNumber::new_i32(0x20),
                &CFNumber::new_i32(0x8A),
            ],
        );
        // SAFETY: the dictionary maps CFString keys to CFNumber values, as
        // IOKit expects.
        unsafe { manager.set_device_matching(Some(matching.as_opaque())) };
        if manager.open(0) != kIOReturnSuccess {
            return None;
        }
        let devices = manager.devices()?;
        if devices.count() < 1 {
            return None;
        }
        let mut values: Vec<*const c_void> =
            vec![std::ptr::null(); usize::try_from(devices.count()).ok()?];
        // SAFETY: `values` has room for every element of the set.
        unsafe { devices.values(values.as_mut_ptr()) };
        let first = NonNull::new(values[0].cast_mut().cast::<IOHIDDevice>())?;
        // SAFETY: the set holds IOHIDDevice references; retain one for us.
        let device = unsafe { CFRetained::retain(first) };
        if device.open(0) != kIOReturnSuccess {
            return None;
        }
        Some(Self {
            _manager: manager,
            device,
        })
    }

    /// The lid angle in degrees.
    fn angle(&self) -> Option<u16> {
        let mut report = [0u8; 8];
        let mut length: CFIndex = report.len().try_into().ok()?;
        // SAFETY: both pointers point to live, correctly sized buffers.
        let status = unsafe {
            self.device.report(
                IOHIDReportType::Feature,
                1,
                NonNull::new_unchecked(report.as_mut_ptr()),
                NonNull::from(&mut length),
            )
        };
        (status == kIOReturnSuccess && length >= 3)
            .then(|| u16::from_le_bytes([report[1], report[2]]))
    }
}

/// Turns angles into the animation's progress: 0 at rest, 1 nearly shut.
/// It is relative to where the lid usually rests, so it starts as soon as
/// the lid begins to close from any working angle.
#[derive(Debug, Default)]
struct Tracker {
    /// The angle the lid rests at, once known.
    rest: Option<f64>,
    /// A lower angle the lid has stayed at, and since when.
    settling: Option<(u16, Instant)>,
}

impl Tracker {
    /// How far below the resting angle the effect starts.
    const SLACK: f64 = 6.0;
    /// The effect never starts above this, so shifting a lid that rests
    /// wide open, on a couch or in bed, does not set it off.
    const HIGHEST_START: f64 = 90.0;
    /// The angle at which the effect is complete.
    const SHUT: f64 = 8.0;
    /// Below this, the lid is closing rather than resting.
    const LOWEST_REST: u16 = 45;
    /// How long the lid has to stay put at a lower angle to rest there.
    const SETTLE: Duration = Duration::from_secs(3);

    /// Feeds a reading taken at `now` and returns the progress.
    fn update(&mut self, angle: u16, now: Instant) -> f64 {
        let value = f64::from(angle);
        match self.rest {
            // Raising the lid rests it higher right away.
            Some(rest) if value <= rest => {
                if angle >= Self::LOWEST_REST {
                    match self.settling {
                        Some((at, since)) if at == angle => {
                            if now.duration_since(since) >= Self::SETTLE {
                                self.rest = Some(value);
                                self.settling = None;
                            }
                        }
                        _ => self.settling = Some((angle, now)),
                    }
                } else {
                    self.settling = None;
                }
            }
            _ => {
                self.rest = Some(value.max(f64::from(Self::LOWEST_REST)));
                self.settling = None;
            }
        }
        self.progress(value)
    }

    fn progress(&self, angle: f64) -> f64 {
        let rest = self.rest.unwrap_or_else(|| f64::from(Self::LOWEST_REST));
        let start = (rest - Self::SLACK).min(Self::HIGHEST_START);
        ((start - angle) / (start - Self::SHUT)).clamp(0.0, 1.0)
    }
}

/// Eases the shown progress toward the sensor's, so whole-degree readings
/// become a continuous movement.
#[derive(Debug, Default)]
struct Smoother {
    value: f64,
}

impl Smoother {
    /// How quickly it follows, as a time constant.
    const LAG: f64 = 0.07;

    fn step(&mut self, target: f64, dt: Duration) -> f64 {
        let k = 1.0 - (-dt.as_secs_f64() / Self::LAG).exp();
        self.value += (target - self.value) * k;
        if (target - self.value).abs() < 0.001 {
            self.value = target;
        }
        self.value
    }
}

/// Polls the sensor on a background thread and hands the progress to
/// `lid_view` on the main thread. Fast while the lid moves or the effect
/// shows, slow while it rests.
pub fn watch() {
    let Some(sensor) = LidSensor::open() else {
        tracing::info!("no lid sensor; the lid animation is off");
        return;
    };
    let spawned = std::thread::Builder::new()
        .name("obojeno-lid".into())
        .spawn(move || {
            let mut tracker = Tracker::default();
            let mut smoother = Smoother::default();
            let mut target = 0.0;
            let mut sent = 0.0;
            let mut last_angle = None;
            let mut last_tick = Instant::now();
            loop {
                let now = Instant::now();
                let angle = sensor.angle();
                if let Some(angle) = angle {
                    target = tracker.update(angle, now);
                }
                let shown = smoother.step(target, now - last_tick);
                last_tick = now;
                if (shown - sent).abs() > f64::EPSILON {
                    sent = shown;
                    dispatch2::DispatchQueue::main().exec_async(move || {
                        if let Some(mtm) = MainThreadMarker::new() {
                            crate::lid_view::progress(shown, mtm);
                        }
                    });
                }
                let moving = angle != last_angle;
                last_angle = angle;
                // A frame at 120 Hz while it glides, quick polls while the
                // lid moves or the effect shows, slow ones at rest.
                let pause = if (shown - target).abs() > f64::EPSILON {
                    8
                } else if moving || shown > 0.0 {
                    16
                } else {
                    250
                };
                std::thread::sleep(Duration::from_millis(pause));
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("cannot watch the lid: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_starts_below_the_resting_angle() {
        let now = Instant::now();
        let mut lid = Tracker::default();
        assert!(lid.update(101, now) < f64::EPSILON);
        assert!(lid.update(96, now) < f64::EPSILON, "within the slack");
        let half = lid.update(49, now);
        assert!((half - 41.0 / 82.0).abs() < 1e-9, "{half}");
        assert!(lid.update(110, now) < f64::EPSILON);
        assert!(lid.update(92, now) < f64::EPSILON, "never above 90°");
        assert!(lid.update(89, now) > 0.0);
        assert!((lid.update(8, now) - 1.0).abs() < f64::EPSILON);
        // Opening again reveals it back up to where it rested.
        assert!(lid.update(95, now) < f64::EPSILON);
    }

    #[test]
    fn rests_lower_only_after_staying_there() {
        let start = Instant::now();
        let later = |s| start + Duration::from_secs(s);
        let mut lid = Tracker::default();
        lid.update(110, start);
        assert!(lid.update(80, later(1)) > 0.0);
        assert!(lid.update(80, later(3)) > 0.0, "not settled yet");
        assert!(lid.update(80, later(4)) < f64::EPSILON, "settled at 80°");
        assert!(lid.update(70, later(5)) > 0.0);
        // Held halfway shut, it is still closing, not resting.
        lid.update(30, later(6));
        assert!((lid.update(30, later(60)) - lid.update(30, later(61))).abs() < f64::EPSILON);
        assert!(lid.update(30, later(62)) > 0.5);
    }

    #[test]
    fn first_seen_closing_assumes_the_lowest_rest() {
        let now = Instant::now();
        let mut lid = Tracker::default();
        let t = lid.update(20, now);
        assert!((t - 19.0 / 31.0).abs() < 1e-9, "{t}");
        assert!(lid.update(40, now) < f64::EPSILON);
    }

    #[test]
    fn smoother_glides_to_the_target() {
        let frame = Duration::from_millis(8);
        let mut smoother = Smoother::default();
        let first = smoother.step(1.0, frame);
        assert!(first > 0.0 && first < 0.2, "{first}");
        let mut previous = first;
        for _ in 0..200 {
            let next = smoother.step(1.0, frame);
            assert!(next >= previous);
            previous = next;
        }
        assert!((previous - 1.0).abs() < f64::EPSILON, "reaches it exactly");
    }

    #[test]
    fn reads_a_plausible_angle_when_present() {
        // Runs on any Mac: without the sensor there is nothing to check.
        if let Some(sensor) = LidSensor::open() {
            let angle = sensor.angle().expect("the sensor answers");
            eprintln!("lid angle: {angle}°");
            assert!(angle <= 360, "{angle}");
        }
    }
}
