use defmt::{error, info};
use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::Delay;
use embedded_hal::i2c::ErrorType;
use embedded_hal_async::i2c::I2c;
use femtopb::UnknownFields;
use libscd::asynchronous::scd30::Scd30;
use meshtastic_protobufs::meshtastic::EnvironmentMetrics;

use crate::{TelemetrySensor, environmental_telemetry::EnvironmentData};

/// Alias `Scd30` typedef for the shorter name `SCD30`
pub type SCD30<'dev, BUS> = Scd30<I2cDevice<'dev, CriticalSectionRawMutex, BUS>, Delay>;

/// Implement `TelemetrySensor` on the `SCD30`
impl<'dev, BUS: I2c + ErrorType + Send + 'static> TelemetrySensor<SCD30<'dev, BUS>> {
    /// Creates a new [`TelemetrySensor<SCD30<'dev, BUS>>`].
    ///
    /// # Arguments
    /// * `bus` - An [`I2cDevice`] type implemented on an `BUS` with the [`I2c`] trait
    ///
    /// # Returns
    /// * `Self` - A [`TelemetrySensor`] for a `SCD30`
    #[must_use]
    #[inline]
    pub fn new(bus: I2cDevice<'dev, CriticalSectionRawMutex, BUS>) -> Self {
        Self {
            device: Scd30::new(bus, Delay),
        }
    }
}

/// Implement [`EnvironmentData`] for `SCD30`
impl<BUS: I2c + ErrorType + Send> EnvironmentData for TelemetrySensor<SCD30<'static, BUS>>
where
    <BUS as ErrorType>::Error: defmt::Format,
{
    #[inline]
    async fn get_metrics(&mut self) -> Option<EnvironmentMetrics<'_>> {
        if self.device.data_ready().await.is_ok_and(|b| b) {
            match self.device.read_measurement().await {
                Ok(data) => {
                    info!(
                        "SCD30 get_metrics()\n\t\t Temperature: {:?}\n\t\t Humidity: {:?}",
                        data.temperature, data.humidity
                    );
                    Some(EnvironmentMetrics {
                        temperature: Some(data.temperature),
                        relative_humidity: Some(data.humidity),
                        barometric_pressure: None,
                        gas_resistance: None,
                        voltage: None,
                        current: None,
                        iaq: None,
                        distance: None,
                        lux: None,
                        white_lux: None,
                        ir_lux: None,
                        uv_lux: None,
                        wind_direction: None,
                        wind_speed: None,
                        weight: None,
                        wind_gust: None,
                        wind_lull: None,
                        radiation: None,
                        rainfall_1h: None,
                        rainfall_24h: None,
                        soil_moisture: None,
                        soil_temperature: None,
                        unknown_fields: UnknownFields::default(),
                    })
                }
                Err(err) => {
                    error!("Could not get measurements from SCD30: {:?}", err);
                    None
                }
            }
        } else {
            None
        }
    }
    #[inline]
    async fn setup(&mut self) {}
}
