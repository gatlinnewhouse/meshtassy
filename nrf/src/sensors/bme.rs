use bosch_bme680::{AsyncBme680, BmeError};
use defmt::{Formatter, *};
use embassy_time::Delay;
use femtopb::UnknownFields;
use meshtastic_protobufs::meshtastic::EnvironmentMetrics;

use crate::{
    boards::I2CSensor,
    environmental_telemetry::EnvironmentData,
    sensors::{RemoteError, TelemetrySensor},
};

/// Alias BME typedef for shorter name
type BME<'dev> = AsyncBme680<I2CSensor<'dev>, Delay>;

/// Alias BME Error typeddef for shorter name
type BMEError<'dev> = BmeError<I2CSensor<'dev>>;

/// Implement defmt for the remote crate error struct
impl defmt::Format for RemoteError<BMEError<'_>> {
    fn format(&self, fmt: Formatter) {
        match self.error {
            BmeError::WriteError(e) => defmt::write!(fmt, "Write Error: {:#?}", e),
            BmeError::WriteReadError(e) => defmt::write!(fmt, "Write Read Error: {:#?}", e),
            BmeError::UnexpectedChipId(e) => defmt::write!(fmt, "Unexpected Chip ID: {}", e),
            BmeError::MeasuringTimeOut => defmt::write!(fmt, "Measuring Timeout"),
            BmeError::Uninitialized => defmt::write!(fmt, "Uninitialized"),
        }
    }
}

/// Implement EnvironmentData for BME
impl EnvironmentData for TelemetrySensor<BME<'static>> {
    async fn setup(&mut self) {
        let cfg = bosch_bme680::Configuration::default();
        match self.device.initialize(&cfg).await {
            Ok(_) => info!("BME680 Configured"),
            Err(e) => {
                let re = RemoteError::<BMEError> { error: e };
                error!("Error configuring BME680: {:?}", re)
            }
        }
    }
    async fn get_metrics(&mut self) -> Option<EnvironmentMetrics<'_>> {
        match self.device.measure().await {
            Ok(data) => {
                //TODO: a macro for multiline info messages to make this less annoying
                info!("BME680 get_metrics()\n\t\t Temperature: {:?}\n\t\t Humidity: {:?}\n\t\t Pressure: {:?}\n\t\t Gas Resistance: {:?}\n\t\t IAQ: N/A", data.temperature, data.humidity, data.pressure, data.gas_resistance);
                Some(EnvironmentMetrics {
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
                })
            }
            Err(e) => {
                let re = RemoteError::<BMEError> { error: e };
                error!("Error fetching data from BME: {:?}", re);
                None
            }
        }
    }
}
