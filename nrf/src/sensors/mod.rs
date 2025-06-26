/// BME680 Sensor
mod bme;

/// SCD30 Sensor
mod scd30;

/// Proxy struct for remote device structs
pub struct TelemetrySensor<T> {
    pub device: T,
}

/// Proxy struct for remote device errors that lack defmt support
struct RemoteError<E> {
    error: E,
}
