use defmt::*;
use embassy_time::Delay;
use femtopb::UnknownFields;
use libscd::asynchronous::scd30::Scd30;
use meshtastic_protobufs::meshtastic::EnvironmentMetrics;

use crate::{
    boards::I2CSensor, environmental_telemetry::EnvironmentData, sensors::TelemetrySensor,
};

/// Alias SCD30 typedef for shorter name
type SCD30<'dev> = Scd30<I2CSensor<'dev>, Delay>;

/// Implement EnvironmentData for SCD30
impl EnvironmentData for TelemetrySensor<SCD30<'static>> {
    async fn setup(&mut self) {
        // not much is required initially here. perhaps eventually this runs the calibration routine
    }
    async fn get_metrics(&mut self) -> Option<EnvironmentMetrics<'_>> {
        if self.device.data_ready().await.is_ok_and(|b| b == true) {
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
