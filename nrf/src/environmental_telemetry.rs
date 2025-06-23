use bosch_bme680::{AsyncBme680, BmeError};
use defmt::*;
use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_nrf::{peripherals::TWISPI1, twim::Twim};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::Delay;
use femtopb::UnknownFields;
use libscd::asynchronous::scd30::Scd30;
use meshtastic_protobufs::meshtastic::EnvironmentMetrics;

/// Dummy trait for narrowing proxied remote structs from crates
pub trait CrateSensor {}

/// Proxy struct for remote device structs
pub struct TelemetrySensor<T: CrateSensor> {
    pub device: T,
}

/// Dummy trait for narrowing proxied remote errors from crates
pub trait CrateError {}

/// Proxy struct for remote device errors that lack defmt support
struct RemoteError<E: CrateError> {
    error: E,
}

/// Trait for environmental telemetry data sources
pub trait EnvironmentData {
    async fn setup(&mut self) {}
    async fn get_metrics(&mut self) -> Option<EnvironmentMetrics<'_>> {
        None
    }
}

/// Alias BME typedef for shorter name
type BME<'dev> = AsyncBme680<I2cDevice<'dev, NoopRawMutex, Twim<'dev, TWISPI1>>, Delay>;
/// Implement the dummy trait on the BME struct
impl<'dev> CrateSensor for BME<'dev> {}

/// Alias BME Error typeddef for shorter name
type BMEError<'dev> = BmeError<I2cDevice<'dev, NoopRawMutex, Twim<'dev, TWISPI1>>>;
/// Implement the dummy trait on the BMEError struct
impl<'dev> CrateError for BMEError<'dev> {}
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
                info!("BME680 get_metrics()\n");
                info!("Temperature: {:?}", data.temperature);
                info!("Humidity: {:?}%", data.humidity);
                info!("Pressure: {:?}", data.pressure);
                if let Some(gr) = data.gas_resistance {
                    info!("Gas Resistance: {:?}", gr);
                }
                info!("IAQ: N/A");
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

/// Alias SCD30 typedef for shorter name
type SCD30<'dev> = Scd30<I2cDevice<'dev, NoopRawMutex, Twim<'dev, TWISPI1>>, Delay>;
impl<'dev> CrateSensor for SCD30<'dev> {}

/// Implement EnvironmentData for SCD30
impl EnvironmentData for TelemetrySensor<SCD30<'static>> {
    async fn setup(&mut self) {
        // not much is required initially here. perhaps eventually this runs the calibration routine
    }
    async fn get_metrics(&mut self) -> Option<EnvironmentMetrics<'_>> {
        if self.device.data_ready().await.is_ok_and(|b| b == true) {
            match self.device.read_measurement().await {
                Ok(data) => {
                    info!("Temperature: {:?}", data.temperature);
                    info!("Humidity: {:?}", data.humidity);
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
                Err(e) => {
                    error!("Could not get measurements from SCD30: {:?}", e);
                    None
                }
            }
        } else {
            None
        }
    }
}
