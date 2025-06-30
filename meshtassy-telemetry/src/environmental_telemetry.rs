use meshtastic_protobufs::meshtastic::EnvironmentMetrics;

/// Trait for environmental telemetry data sources
pub trait EnvironmentData {
    /// Setup a given [`TelemetrySensor`] struct that has a sensor on the I2C bus by calling any
    /// required functions of the sensor's implementation
    ///
    /// # Arguments
    /// * `&mut self` - a [`TelemetrySensor`] struct, to use `self.device` to setup sensors
    ///
    /// # Returns
    /// * `Future<Output = ()>` - An `async` block that embassy's executor can use
    ///
    /// # Side-effects
    /// * Possibly delays device or performs some other state management to setup a sensor
    #[inline]
    fn setup(&mut self) -> impl Future<Output = ()> {
        async {}
    }
    /// Get metrics from a given I2C sensor on the [`TelemetrySensor`] type in order to assemble an
    /// [`EnvironmentMetrics`] protobuf part
    ///
    /// # Arguments
    /// * `&mut self` - a [`TelemetrySensor`] struct to assemble the telemetry data
    ///
    /// # Returns
    /// * `Future<Output = Option<EnvironmentMetrics<'_>>>` - either `None` or a
    ///   [`EnvironmentMetrics`] struct for protobufs
    #[inline]
    fn get_metrics(&mut self) -> impl Future<Output = Option<EnvironmentMetrics<'_>>> {
        async { None }
    }
}
