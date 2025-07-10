use crate::{Box, Sensor, SensorVariants};
use async_trait::async_trait;
use defmt::{error, info};
use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::Delay;
use embedded_hal::i2c::ErrorType;
use embedded_hal_async::i2c::I2c;
use femtopb::UnknownFields;
use libscd::asynchronous::scd30::Scd30;
use meshtastic_protobufs::meshtastic::{AirQualityMetrics, EnvironmentMetrics, telemetry::Variant};

/// Alias `Scd30` typedef for the shorter name `SCD30`
pub type SCD30<'dev, BUS> = Scd30<I2cDevice<'dev, CriticalSectionRawMutex, BUS>, Delay>;

#[must_use]
pub fn new_scd30<BUS: I2c + ErrorType + Send + 'static>(
    bus: I2cDevice<'static, CriticalSectionRawMutex, BUS>,
) -> Box<dyn Sensor + 'static>
where
    <BUS as embedded_hal::i2c::ErrorType>::Error: defmt::Format,
{
    Box::new(Scd30::new(bus, Delay))
}

#[async_trait(?Send)]
impl<BUS: I2c + ErrorType + Send> Sensor for SCD30<'static, BUS>
where
    <BUS as ErrorType>::Error: defmt::Format,
{
    async fn setup(&mut self) {}
    async fn get_metrics(&mut self, kind: crate::SensorVariants) -> Option<Variant<'_>> {
        match kind {
            SensorVariants::AirQuality => {
                if self.data_ready().await.is_ok_and(|b| b) {
                    match self.read_measurement().await {
                        Ok(data) => {
                            info!("SCD30 AirQuality get_metrics()\n\t\t CO2: {:?}", data.co2,);
                            Some(Variant::AirQualityMetrics(AirQualityMetrics {
                                pm10_standard: None,
                                pm25_standard: None,
                                pm100_standard: None,
                                pm10_environmental: None,
                                pm25_environmental: None,
                                pm100_environmental: None,
                                particles_03um: None,
                                particles_05um: None,
                                particles_10um: None,
                                particles_25um: None,
                                particles_50um: None,
                                particles_100um: None,
                                co2: Some(data.co2.into()),
                                unknown_fields: UnknownFields::default(),
                            }))
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
            SensorVariants::Environmental => {
                if self.data_ready().await.is_ok_and(|b| b) {
                    match self.read_measurement().await {
                        Ok(data) => {
                            info!(
                                "SCD30 Environmental get_metrics()\n\t\t Temperature: {:?}\n\t\t Humidity: {:?}",
                                data.temperature, data.humidity
                            );
                            Some(Variant::EnvironmentMetrics(EnvironmentMetrics {
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
                            }))
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
        }
    }
}
