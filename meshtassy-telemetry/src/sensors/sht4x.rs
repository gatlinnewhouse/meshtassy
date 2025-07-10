use crate::{Box, Sensor, SensorVariants};
use async_trait::async_trait;
use defmt::{error, info};
use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::Delay;
use embedded_hal::i2c::ErrorType;
use embedded_hal_async::i2c::I2c;
use femtopb::UnknownFields;
use meshtastic_protobufs::meshtastic::{EnvironmentMetrics, telemetry::Variant};
use sht4x::{Precision, Sht4xAsync};

/// Alias `Scd30` typedef for the shorter name `SCD30`
pub type SHT4X<'dev, BUS> = Sht4xAsync<I2cDevice<'dev, CriticalSectionRawMutex, BUS>, Delay>;

#[must_use]
pub fn new_sht4x<BUS: I2c + ErrorType + Send + 'static>(
    bus: I2cDevice<'static, CriticalSectionRawMutex, BUS>,
) -> Box<dyn Sensor + 'static>
where
    <BUS as embedded_hal::i2c::ErrorType>::Error: defmt::Format,
{
    Box::new(Sht4xAsync::new(bus))
}

#[async_trait(?Send)]
impl<BUS: I2c + ErrorType + Send> Sensor for SHT4X<'static, BUS>
where
    <BUS as ErrorType>::Error: defmt::Format,
{
    async fn setup(&mut self) {}
    async fn get_metrics(&mut self, kind: crate::SensorVariants) -> Option<Variant<'_>> {
        match kind {
            SensorVariants::Environmental => match self.measure(Precision::Low, &mut Delay).await {
                Ok(data) => {
                    let temp = data.temperature_celsius().to_num();
                    let humid = data.humidity_percent().to_num();
                    info!(
                        "SHT4X Environmental get_metrics()\n\t\t Temperature: {:?}\n\t\t Humidity: {:?}",
                        temp, humid
                    );
                    Some(Variant::EnvironmentMetrics(EnvironmentMetrics {
                        temperature: Some(temp),
                        relative_humidity: Some(humid),
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
                    }))
                }
                Err(err) => {
                    error!("Could not get measurements from SHT4X: {:?}", err);
                    None
                }
            },
            _ => None,
        }
    }
}
