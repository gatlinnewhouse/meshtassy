#![no_std]
#![deny(clippy::all)]
#![warn(clippy::pedantic)]
#![warn(clippy::nursery)]
//#![warn(clippy::cargo)]

/// Sensor specific code
pub mod sensors;

/// Environmental Telemetry code
pub mod environmental_telemetry;

/// Proxy struct for remote device structs
pub struct TelemetrySensor<T: Send + Sync> {
    /// Device on an I2C bus possibly from a crate
    pub device: T,
}

/// Proxy struct for remote crate errors in order to add defmt support
struct RemoteError<E> {
    /// Error type from remote crate
    error: E,
}
