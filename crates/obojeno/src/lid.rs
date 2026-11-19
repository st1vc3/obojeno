//! The MacBook lid-angle sensor ("las" on the Apple SPU, HID usage page 0x20,
//! usage 0x8A). Its feature report 1 holds the angle in degrees as a
//! little-endian u16: 0 closed, about 100-130 in use. Reading it needs no
//! permission. Desktop Macs and older MacBooks have no sensor.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2_core_foundation::{CFDictionary, CFIndex, CFNumber, CFRetained, CFString};
use objc2_io_kit::{IOHIDDevice, IOHIDManager, IOHIDReportType, kIOReturnSuccess};

pub struct LidSensor {
    // The manager owns the matched device; keep it alive.
    _manager: CFRetained<IOHIDManager>,
    device: CFRetained<IOHIDDevice>,
}

// SAFETY: IOHIDDevice reports can be read from any thread; the sensor is
// only ever used by one thread at a time.
unsafe impl Send for LidSensor {}

impl LidSensor {
    /// The sensor, if this Mac has one.
    pub fn open() -> Option<Self> {
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
    pub fn angle(&self) -> Option<u16> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
