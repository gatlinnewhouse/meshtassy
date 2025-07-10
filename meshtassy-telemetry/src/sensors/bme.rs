use crate::{Box, RemoteError, Sensor, SensorVariants, TelemetrySensor};
use async_trait::async_trait;
use bosch_bme680::{AsyncBme680, BmeError};
use defmt::{Formatter, error, info};
use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::Delay;
use embedded_hal::i2c::ErrorType;
use embedded_hal_async::i2c::I2c;
use femtopb::UnknownFields;
use meshtastic_protobufs::meshtastic::{EnvironmentMetrics, telemetry::Variant};

/// Alias `AsyncBme680` typedef for the shorter name `BME`
pub type BME<'dev, BUS> = AsyncBme680<I2cDevice<'dev, CriticalSectionRawMutex, BUS>, Delay>;

/// Alias `BmeError` typeddef for the shorter name `BMEError`
type BMEError<'dev, BUS> = BmeError<I2cDevice<'dev, CriticalSectionRawMutex, BUS>>;

/// Implement defmt for the remote crate error struct
impl<BUS: I2c + ErrorType> defmt::Format for RemoteError<BMEError<'_, BUS>>
where
    <BUS as ErrorType>::Error: defmt::Format,
{
    #[inline]
    fn format(&self, fmt: Formatter) {
        match &self.error {
            BmeError::WriteError(err) => defmt::write!(fmt, "Write Error: {:#?}", err),
            BmeError::WriteReadError(err) => defmt::write!(fmt, "Write Read Error: {:#?}", err),
            BmeError::UnexpectedChipId(err) => defmt::write!(fmt, "Unexpected Chip ID: {}", err),
            BmeError::MeasuringTimeOut => defmt::write!(fmt, "Measuring Timeout"),
            BmeError::Uninitialized => defmt::write!(fmt, "Uninitialized"),
        }
    }
}

#[must_use]
pub fn new_bme<BUS: I2c + ErrorType + Send + 'static>(
    bus: I2cDevice<'static, CriticalSectionRawMutex, BUS>,
) -> Box<dyn Sensor + 'static>
where
    <BUS as embedded_hal::i2c::ErrorType>::Error: defmt::Format,
{
    Box::new(bosch_bme680::AsyncBme680::new(
        bus,
        bosch_bme680::DeviceAddress::Secondary,
        Delay,
        24, // wrong initial temperature, is it in C?
    ))
}

#[async_trait(?Send)]
impl<BUS: I2c + ErrorType + Send> Sensor for BME<'static, BUS>
where
    <BUS as embedded_hal::i2c::ErrorType>::Error: defmt::Format,
{
    async fn setup(&mut self) {
        let cfg = bosch_bme680::Configuration::default();
        match self.initialize(&cfg).await {
            Ok(_a) => info!("BME680 Configured"),
            Err(err) => {
                let re = RemoteError::<BMEError<BUS>> { error: err };
                error!("Error configuring BME680: {:?}", re);
            }
        }
    }
    async fn get_metrics(&mut self, kind: crate::SensorVariants) -> Option<Variant<'_>> {
        match kind {
            SensorVariants::Environmental => {
                match self.measure().await {
                    Ok(data) => {
                        //TODO: a macro for multiline info messages to make this less annoying
                        info!(
                            "BME680 Environmental get_metrics()\n\t\t Temperature: {:?}\n\t\t Humidity: {:?}\n\t\t Pressure: {:?}\n\t\t Gas Resistance: {:?}\n\t\t IAQ: N/A",
                            data.temperature, data.humidity, data.pressure, data.gas_resistance
                        );
                        Some(Variant::EnvironmentMetrics(EnvironmentMetrics {
                            temperature: Some(data.temperature),
                            relative_humidity: Some(data.humidity),
                            barometric_pressure: Some(data.pressure),
                            gas_resistance: data.gas_resistance,
                            voltage: None,
                            current: None,
                            iaq: None, // C++ firmware shows IAQ from a BME, perhaps this crate is not great
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
                    Err(e) => {
                        let re = RemoteError::<BMEError<BUS>> { error: e };
                        error!("Error fetching data from BME: {:?}", re);
                        None
                    }
                }
            }
            _ => None,
        }
    }
}
