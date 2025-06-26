use meshtastic_protobufs::meshtastic::EnvironmentMetrics;

/// Trait for environmental telemetry data sources
pub trait EnvironmentData {
    async fn setup(&mut self) {}
    async fn get_metrics(&mut self) -> Option<EnvironmentMetrics<'_>> {
        None
    }
}
