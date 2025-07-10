#![no_std]
#![deny(clippy::all)]
#![warn(clippy::pedantic)]
#![warn(clippy::nursery)]
//#![warn(clippy::cargo)]

use alloc::boxed::Box;
use alloc::vec::Vec;
use async_trait::async_trait;
use meshtastic_protobufs::meshtastic::telemetry::Variant;

// Use alloc crate
extern crate alloc;

/// Sensor specific code
pub mod sensors;

/// Environmental Telemetry code
pub mod environmental_telemetry;

/// Possible telemetry data kinds from a sensor
pub enum SensorVariants {
    AirQuality,
    Environmental,
}

/// The trait required of any I2C sensor providing data
#[async_trait(?Send)]
pub trait Sensor: Sync {
    async fn setup(&mut self);
    async fn get_metrics(&mut self, kind: SensorVariants) -> Option<Variant<'_>>;
}

/// A list of Sensors
pub struct Sensors {
    sensors: Vec<TelemetrySensor>,
    idx: usize,
}

impl Sensors {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sensors: Vec::new(),
            idx: 0,
        }
    }
    pub fn add(&mut self, device: TelemetrySensor) {
        self.sensors.push(device);
    }
    pub async fn get_metrics(&mut self, kind: SensorVariants) -> Option<Variant<'_>> {
        let last = self.sensors.len() - 1;
        match self.sensors.get_mut(self.idx) {
            Some(sensor) => {
                if self.idx == last {
                    self.idx = 0;
                } else {
                    self.idx += 1;
                }
                sensor.device.get_metrics(kind).await
            }
            None => None,
        }
    }
}

impl Default for Sensors {
    fn default() -> Self {
        Self::new()
    }
}

/// Proxy struct for remote device structs
pub struct TelemetrySensor {
    /// Device on an I2C bus possibly from a crate
    pub device: Box<dyn Sensor>,
}

/// Proxy struct for remote crate errors in order to add defmt support
struct RemoteError<E> {
    /// Error type from remote crate
    error: E,
}
