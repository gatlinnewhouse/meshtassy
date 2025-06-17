//! Packet relay and retransmission logic for Meshtastic
//!
//! This module implements the packet relay system that:
//! 1. Listens to received packets from the packet channel
//! 2. Decides whether packets should be relayed
//! 3. Implements timing logic for retransmissions
//! 4. Manages channel activity detection before transmission

use defmt::*;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Instant, Timer};
use heapless::spsc::Queue;
use heapless::{FnvIndexMap, Vec};
use meshtassy_net::DecodedPacket;
use meshtastic_protobufs::meshtastic::PortNum;

use crate::PACKET_CHANNEL;

/// Maximum number of pending packets for retransmission
const MAX_PENDING_PACKETS: usize = 16;

/// Default number of retransmissions for reliable packets
const NUM_RELIABLE_RETX: u8 = 3;

/// Number of retransmissions for ACK/routing packets (more critical)
const NUM_ACK_RETX: u8 = 4;

/// Number of retransmissions for text messages
const NUM_TEXT_RETX: u8 = 2;

/// Minimum hop delay in milliseconds
const MIN_HOP_DELAY_MS: u64 = 200;

/// Maximum hop delay in milliseconds
const MAX_HOP_DELAY_MS: u64 = 300;

/// Additional delay per hop in milliseconds
const HOP_PENALTY_MS: u64 = 50;

/// Retransmission intervals in milliseconds (exponential backoff)
const RETX_INTERVALS: [u64; 5] = [5000, 15000, 30000, 60000, 120000];

/// Channel busy backoff: base delay in milliseconds
const CHANNEL_BUSY_BASE_DELAY_MS: u64 = 100;

/// Channel busy backoff: maximum retry count (2^8 = 256 seconds max delay)
const MAX_CHANNEL_BUSY_RETRIES: u8 = 8;

/// Maximum time to keep a packet before giving up (60 seconds)
const MAX_PACKET_AGE_MS: u64 = 60_000;

/// Our node ID (hardcoded for now, should come from config)
const OUR_NODE_ID: u32 = 0xDEADBEEF;

/// Channel utilization limit (10%)
const CHANNEL_UTIL_LIMIT: u8 = 10;

/// Relay statistics for monitoring and observability
#[derive(Default, Clone, Debug)]
pub struct RelayStats {
    /// Total packets received and considered for relay
    pub packets_received: u32,
    /// Total packets queued for relay transmission
    pub packets_queued: u32,
    /// Total packets successfully transmitted
    pub packets_transmitted: u32,
    /// Total packets dropped due to full queue or other errors
    pub packets_dropped: u32,
    /// Total packets expired and removed from queue
    pub packets_expired: u32,
    /// Number of times channel was detected as busy
    pub channel_busy_events: u32,
    /// Number of transmissions blocked by duty cycle limits
    pub duty_cycle_blocks: u32,
    /// Number of implicit ACKs received
    pub implicit_acks: u32,
    /// Number of packets dropped due to loop prevention
    pub loop_prevention_drops: u32,
    /// Number of retransmissions attempted
    pub retransmissions: u32,
    /// Current duty cycle utilization percentage
    pub current_duty_cycle_percent: u8,
}

/// Errors that can occur in the packet relay system
#[derive(Debug, Clone, defmt::Format)]
pub enum RelayError {
    /// Queue is full, cannot add more packets
    QueueFull,
    /// Duty cycle limit would be exceeded
    DutyCycleExceeded,
    /// Channel is too busy for transmission
    ChannelBusy,
    /// Packet transmission failed
    TransmissionFailed(&'static str),
    /// Invalid packet data
    InvalidPacket(&'static str),
    /// System not initialized
    NotInitialized,
}

/// Packet priority constants (matching Meshtastic enum values)
mod priority {
    pub const UNSET: u8 = 0;
    pub const MIN: u8 = 1;
    pub const BACKGROUND: u8 = 10;
    pub const DEFAULT: u8 = 64;
    pub const RELIABLE: u8 = 70;
    pub const ACK: u8 = 120;
    pub const MAX: u8 = 127;
}

/// Calculate delay multiplier from numeric priority (0-127)
/// Higher numeric priority = shorter delay
/// Formula matches Meshtastic's: (127 - priority) / 127 * 0.5 + 0.5
fn priority_delay_multiplier(priority: u8) -> f32 {
    let priority_scale = (127.0 - priority as f32) / 127.0;
    0.5 + priority_scale * 0.5
}

/// Represents a packet pending retransmission
#[derive(Clone)]
struct PendingPacket {
    /// The original packet data
    packet: DecodedPacket,
    /// When this packet was first queued
    queued_at: Instant,
    /// When the next transmission should occur
    next_tx_time: Instant,
    /// Maximum number of retransmissions for this packet
    max_retransmissions: u8,
    /// Current retry attempt (0 = first transmission, 1+ = retransmissions)
    retry_count: u8,
    /// Number of times channel was busy (for exponential backoff)
    channel_busy_count: u8,
    /// Whether this packet wants an ACK
    wants_ack: bool,
    /// Priority of the packet (0-127, higher = more urgent)
    priority: u8,
}

impl PendingPacket {
    /// Calculate remaining retransmissions
    fn retransmissions_left(&self) -> u8 {
        self.max_retransmissions.saturating_sub(self.retry_count)
    }

    /// Check if packet has more retransmissions available
    fn has_retransmissions_left(&self) -> bool {
        self.retry_count < self.max_retransmissions
    }
}

/// State for the packet relay system
struct RelayState {
    /// Packets pending retransmission, keyed by packet ID
    pending_packets: FnvIndexMap<u32, PendingPacket, MAX_PENDING_PACKETS>,
    /// Recently seen packet IDs to prevent loops
    recent_packets: Queue<u32, 32>,
    /// Our random node ID for this session
    our_node_id: u32,
    /// Whether RX boost is enabled (affects timing)
    rx_boost_enabled: bool,
    /// Duty cycle tracker for channel utilization
    duty_cycle: DutyCycleTracker,
    /// Statistics for monitoring and observability
    stats: RelayStats,
}

/// Tracks channel duty cycle to comply with regional regulations
struct DutyCycleTracker {
    /// Total airtime used in current period (milliseconds)
    total_airtime_ms: u32,
    /// When the current measurement period started
    period_start: Instant,
    /// Channel utilization limit (percentage, 0-100)
    utilization_limit: u8,
    /// Length of measurement period in seconds (typically 3600 for 1 hour)
    period_length_secs: u32,
}

impl DutyCycleTracker {
    fn new(utilization_limit: u8) -> Self {
        Self {
            total_airtime_ms: 0,
            period_start: Instant::now(),
            utilization_limit,
            period_length_secs: 3600, // 1 hour period
        }
    }
    /// Check if we can transmit without exceeding duty cycle limits
    /// packet_airtime_ms: estimated airtime for the packet we want to send
    fn can_transmit(&self, packet_airtime_ms: u32) -> bool {
        let now = Instant::now();
        let period_elapsed_ms = now.duration_since(self.period_start).as_millis() as u32;

        // If period has expired, we can transmit (will be reset by caller)
        if period_elapsed_ms >= (self.period_length_secs * 1000) {
            return true;
        }

        // Calculate what our utilization would be after this transmission
        let new_total_airtime = self.total_airtime_ms.saturating_add(packet_airtime_ms);
        let period_length_ms = self.period_length_secs.saturating_mul(1000);
        let new_utilization_percent = (new_total_airtime as u64 * 100) / period_length_ms as u64;

        new_utilization_percent <= self.utilization_limit as u64
    }

    /// Record airtime usage for a transmitted packet
    fn record_transmission(&mut self, packet_airtime_ms: u32) {
        let now = Instant::now();
        let period_elapsed_ms = now.duration_since(self.period_start).as_millis() as u32;

        // Reset period if it has expired
        if period_elapsed_ms >= (self.period_length_secs * 1000) {
            self.total_airtime_ms = packet_airtime_ms;
            self.period_start = now;
        } else {
            self.total_airtime_ms += packet_airtime_ms;
        }
    }

    /// Get current duty cycle utilization as percentage (0-100)
    fn get_utilization_percent(&self) -> u8 {
        let now = Instant::now();
        let period_elapsed_ms = now.duration_since(self.period_start).as_millis() as u32;

        // If period has expired, utilization is 0
        if period_elapsed_ms >= (self.period_length_secs * 1000) {
            return 0;
        }

        let utilization_percent = (self.total_airtime_ms * 100) / (self.period_length_secs * 1000);
        (utilization_percent as u8).min(100)
    }
}

impl RelayState {
    fn new() -> Self {
        Self {
            pending_packets: FnvIndexMap::new(),
            recent_packets: Queue::new(),
            our_node_id: OUR_NODE_ID,
            rx_boost_enabled: true, // Assume boosted for conservative timing
            duty_cycle: DutyCycleTracker::new(CHANNEL_UTIL_LIMIT),
            stats: RelayStats::default(),
        }
    }

    /// Check if we've recently seen this packet (loop prevention)
    fn is_recent_packet(&self, packet_id: u32) -> bool {
        self.recent_packets.iter().any(|&id| id == packet_id)
    }

    /// Add a packet to the recent packets list
    fn add_recent_packet(&mut self, packet_id: u32) {
        if self.recent_packets.is_full() {
            let _ = self.recent_packets.dequeue();
        }
        let _ = self.recent_packets.enqueue(packet_id);
    }
    /// Check if a packet should be relayed
    fn should_relay(&mut self, packet: &DecodedPacket) -> Result<bool, RelayError> {
        self.stats.packets_received += 1;

        debug!(
            "Evaluating packet {} from {:08X} to {:08X} for relay (port: {:?}, hop_limit: {})",
            packet.header.packet_id,
            packet.header.source,
            packet.header.destination,
            packet.port_num(),
            packet.header.flags.hop_limit
        );

        // Validate packet first
        self.validate_packet(packet)?;

        // Don't relay our own packets
        if packet.header.source == self.our_node_id {
            debug!(
                "Skipping relay of packet {} - originated from us ({:08X})",
                packet.header.packet_id, self.our_node_id
            );
            return Ok(false);
        }

        // Don't relay if we've seen this packet recently
        if self.is_recent_packet(packet.header.packet_id) {
            self.stats.loop_prevention_drops += 1;
            info!(
                "Duplicate packet {} detected, skipping relay (loop prevention)",
                packet.header.packet_id
            );
            return Ok(false);
        }

        // Check hop limit
        if packet.header.flags.hop_limit == 0 {
            debug!(
                "Skipping relay of packet {} - hop limit reached (hop_start: {}, hop_limit: {})",
                packet.header.packet_id,
                packet.header.flags.hop_start,
                packet.header.flags.hop_limit
            );
            return Ok(false);
        }
        // Don't relay packets that are addressed directly to us (they've reached their destination)
        if packet.header.destination == self.our_node_id {
            debug!(
                "Skipping relay of packet {} - addressed to us ({:08X}), packet has reached its destination",
                packet.header.packet_id, self.our_node_id
            );
            return Ok(false);
        }

        // Relay broadcast packets (0xFFFFFFFF) and packets addressed to other nodes
        // This helps packets reach their intended destinations through the mesh

        // Check port - some ports shouldn't be relayed
        let should_relay = match packet.port_num() {
            femtopb::EnumValue::Known(PortNum::RoutingApp) => true,
            femtopb::EnumValue::Known(PortNum::TextMessageApp) => true,
            femtopb::EnumValue::Known(PortNum::NodeinfoApp) => true,
            femtopb::EnumValue::Known(PortNum::PositionApp) => true,
            femtopb::EnumValue::Known(PortNum::TelemetryApp) => true,
            _ => {
                debug!(
                    "Skipping relay of packet {} - port {:?} not configured for relay",
                    packet.header.packet_id,
                    packet.port_num()
                );
                false // Be conservative with unknown ports
            }
        };

        if should_relay {
            debug!(
                "Packet {} passed all relay checks - will be queued for relay",
                packet.header.packet_id
            );
        }

        Ok(should_relay)
    }
    /// Validate packet data before processing
    fn validate_packet(&self, packet: &DecodedPacket) -> Result<(), RelayError> {
        // Check for invalid packet ID (0 is reserved)
        if packet.header.packet_id == 0 {
            return Err(RelayError::InvalidPacket("Packet ID cannot be zero"));
        }

        // Check for reasonable hop limits
        if packet.header.flags.hop_start > 7 {
            return Err(RelayError::InvalidPacket("Hop start too high"));
        }

        if packet.header.flags.hop_limit > packet.header.flags.hop_start {
            return Err(RelayError::InvalidPacket(
                "Hop limit cannot exceed hop start",
            ));
        }

        // Check for reserved node IDs
        if packet.header.source == 0 || packet.header.source == 0xFFFFFFFF {
            return Err(RelayError::InvalidPacket("Invalid source node ID"));
        }

        Ok(())
    }

    /// Calculate the initial relay delay for a packet
    fn calculate_relay_delay(&self, packet: &DecodedPacket, priority: u8) -> u64 {
        // Base delay depends on RX boost setting
        let base_delay = if self.rx_boost_enabled {
            MAX_HOP_DELAY_MS
        } else {
            MIN_HOP_DELAY_MS
        };

        // Add penalty for each hop already taken
        let hops_taken = packet
            .header
            .flags
            .hop_start
            .saturating_sub(packet.header.flags.hop_limit);
        let hop_penalty = hops_taken as u64 * HOP_PENALTY_MS; // Apply priority multiplier
        let adjusted_delay =
            ((base_delay + hop_penalty) as f32 * priority_delay_multiplier(priority)) as u64; // Add random jitter (0 to 2x adjusted delay)
                                                                                              // Use a simple PRNG based on current time since fastrand isn't available
        let time_seed = embassy_time::Instant::now().as_millis() as u64;
        let jitter = (time_seed * 1103515245 + 12345) % (2 * adjusted_delay + 1);

        adjusted_delay + jitter
    }
    /// Add a packet for potential relay
    fn queue_packet(
        &mut self,
        packet: DecodedPacket,
        wants_ack: bool,
        priority: u8,
    ) -> Result<(), RelayError> {
        let now = Instant::now();
        let delay_ms = self.calculate_relay_delay(&packet, priority);
        let max_retransmissions = get_max_retransmissions(&packet, wants_ack);
        let pending = PendingPacket {
            packet: packet.clone(),
            queued_at: now,
            next_tx_time: now + Duration::from_millis(delay_ms),
            max_retransmissions,
            retry_count: 0,
            channel_busy_count: 0,
            wants_ack,
            priority,
        };

        // Store in pending packets map
        let packet_id = packet.header.packet_id;
        if self.pending_packets.insert(packet_id, pending).is_err() {
            self.stats.packets_dropped += 1;
            warn!("Failed to queue packet {} - pending queue full", packet_id);
            return Err(RelayError::QueueFull);
        } else {
            self.stats.packets_queued += 1;
            let queue_pos = self.pending_packets.len();
            info!(
                "Packet {} will be relayed after a delay of {}ms (priority: {}, max_retransmits {}), queue position: {}",
                packet_id, delay_ms, priority, max_retransmissions, queue_pos
            );
        }

        // Mark as recently seen
        self.add_recent_packet(packet_id);
        Ok(())
    }
    /// Handle implicit ACK (when we hear our broadcast echoed)
    fn handle_implicit_ack(&mut self, packet: &DecodedPacket) {
        // If we hear someone else rebroadcasting our packet, consider it acknowledged
        if packet.header.source == self.our_node_id {
            if self.pending_packets.contains_key(&packet.header.packet_id) {
                info!(
                    "Implicit ACK received for packet {} - removing from pending queue",
                    packet.header.packet_id
                );
                self.pending_packets.remove(&packet.header.packet_id);
                self.stats.implicit_acks += 1;
            } else {
                debug!(
                    "Received our own packet {} but it was not in pending queue (already completed or never relayed)",
                    packet.header.packet_id
                );
            }
        }
    }
    /// Remove expired packets
    fn cleanup_expired_packets(&mut self) {
        let now = Instant::now();
        let mut to_remove = Vec::<u32, MAX_PENDING_PACKETS>::new();

        for (packet_id, pending) in &self.pending_packets {
            if now.duration_since(pending.queued_at).as_millis() > MAX_PACKET_AGE_MS {
                let _ = to_remove.push(*packet_id);
            }
        }

        for packet_id in to_remove {
            self.pending_packets.remove(&packet_id);
            self.stats.packets_expired += 1;
            warn!("Removed expired packet {}", packet_id);
        }
    }

    /// Get the next packet ready for transmission
    fn get_next_ready_packet(&mut self) -> Option<(u32, DecodedPacket)> {
        let now = Instant::now();

        // Find the first packet ready for transmission
        for (packet_id, pending) in &self.pending_packets {
            if now >= pending.next_tx_time {
                let packet = pending.packet.clone();
                return Some((*packet_id, packet));
            }
        }

        None
    }
    /// Handle a packet that was just transmitted
    fn handle_transmitted_packet(&mut self, packet_id: u32) {
        if let Some(pending) = self.pending_packets.get_mut(&packet_id) {
            self.stats.packets_transmitted += 1;

            if pending.has_retransmissions_left() {
                // Schedule next retransmission
                pending.retry_count += 1;
                self.stats.retransmissions += 1;

                let retry_interval = if (pending.retry_count as usize) <= RETX_INTERVALS.len() {
                    RETX_INTERVALS[pending.retry_count as usize - 1]
                } else {
                    RETX_INTERVALS[RETX_INTERVALS.len() - 1]
                };

                pending.next_tx_time = Instant::now() + Duration::from_millis(retry_interval);

                debug!(
                    "Scheduled retransmission {} for packet {} in {}ms (retries left: {})",
                    pending.retry_count,
                    packet_id,
                    retry_interval,
                    pending.retransmissions_left()
                );
            } else {
                // No more retransmissions
                self.pending_packets.remove(&packet_id);
                info!("Packet {} transmission complete", packet_id);
            }
        }
    }
    /// Handle exponential backoff when channel is busy
    fn handle_channel_busy(&mut self, packet_id: u32) {
        if let Some(pending) = self.pending_packets.get_mut(&packet_id) {
            // Increment busy count and apply exponential backoff
            pending.channel_busy_count += 1;
            self.stats.channel_busy_events += 1;

            // Calculate exponential backoff: 100ms * 2^count, capped at max retries
            if pending.channel_busy_count <= MAX_CHANNEL_BUSY_RETRIES {
                let backoff_ms = CHANNEL_BUSY_BASE_DELAY_MS * (1 << pending.channel_busy_count);
                pending.next_tx_time = Instant::now() + Duration::from_millis(backoff_ms);

                debug!(
                    "Channel busy for packet {}, backoff attempt {} - delaying {}ms",
                    packet_id, pending.channel_busy_count, backoff_ms
                );
            } else {
                // Too many channel busy events, give up on this packet
                self.pending_packets.remove(&packet_id);
                self.stats.packets_dropped += 1;
                warn!(
                    "Giving up on packet {} - channel busy too many times",
                    packet_id
                );
            }
        }
    }
}

/// Global relay state protected by mutex
static RELAY_STATE: Mutex<CriticalSectionRawMutex, Option<RelayState>> = Mutex::new(None);

/// Initialize a fake random number generator (replace with proper RNG)
fn init_random() {
    // In a real implementation, seed with hardware RNG
    // fastrand::seed(embassy_time::Instant::now().as_millis());
}

/// Check if the channel is currently busy
/// TODO: This should interface with the actual LoRa radio's CAD functionality
fn is_channel_busy() -> bool {
    // Placeholder implementation
    // In a real implementation, this would:
    // 1. Call the LoRa radio's CAD (Channel Activity Detection)
    // 2. Check if we're currently receiving
    // 3. Check duty cycle limits
    false
}

/// Simulate packet transmission
/// TODO: This should interface with the actual LoRa transmitter
async fn transmit_packet(_packet: &DecodedPacket) -> Result<(), &'static str> {
    // Placeholder implementation
    // In a real implementation, this would:
    // 1. Encode the packet back to bytes
    // 2. Encrypt the packet
    // 3. Send via LoRa radio
    info!("Transmitting packet (simulated)");
    Timer::after(Duration::from_millis(100)).await; // Simulate transmission time
    Ok(())
}

/// Calculate estimated airtime for a packet in milliseconds
/// TODO: This should be calculated based on actual packet size, modulation parameters, etc.
fn calculate_packet_airtime(_packet: &DecodedPacket) -> u32 {
    // Placeholder: Return conservative estimate for typical Meshtastic packet
    // Real implementation should consider:
    // - Packet size (header + payload)
    // - LoRa spreading factor, bandwidth, coding rate
    // - Preamble length
    // - Regional variations
    250 // ~250ms for typical packet on LongFast (SF11, BW125)
}

/// Determine packet priority from the decoded packet
fn get_packet_priority(packet: &DecodedPacket) -> u8 {
    match packet.port_num() {
        femtopb::EnumValue::Known(PortNum::RoutingApp) => priority::ACK,
        femtopb::EnumValue::Known(PortNum::TextMessageApp) => priority::DEFAULT,
        _ => priority::DEFAULT,
    }
}

/// Determine maximum retransmissions based on packet type and priority
fn get_max_retransmissions(packet: &DecodedPacket, wants_ack: bool) -> u8 {
    if !wants_ack {
        return 0; // No retransmissions for packets that don't want ACK
    }

    match packet.port_num() {
        femtopb::EnumValue::Known(PortNum::RoutingApp) => NUM_ACK_RETX,
        femtopb::EnumValue::Known(PortNum::TextMessageApp) => NUM_TEXT_RETX,
        _ => NUM_RELIABLE_RETX,
    }
}

/// Check if a packet wants an acknowledgment
fn packet_wants_ack(packet: &DecodedPacket) -> bool {
    // In a real implementation, check the want_response field in the Data
    // For now, assume routing packets want ACK
    matches!(
        packet.port_num(),
        femtopb::EnumValue::Known(PortNum::RoutingApp)
    )
}

/// Check if the relay system is properly initialized
async fn is_relay_initialized() -> bool {
    let relay_state_guard = RELAY_STATE.lock().await;
    relay_state_guard.is_some()
}

/// Get current relay queue status (for debugging and monitoring)
pub async fn get_relay_queue_status() -> (usize, usize, bool) {
    let relay_state_guard = RELAY_STATE.lock().await;
    if let Some(ref relay_state) = *relay_state_guard {
        (
            relay_state.pending_packets.len(),
            relay_state.recent_packets.len(),
            relay_state.duty_cycle.get_utilization_percent() > 90, // Near limit warning
        )
    } else {
        (0, 0, false)
    }
}

/// Reset relay statistics (for testing/monitoring)
pub async fn reset_relay_stats() -> Result<(), RelayError> {
    let mut relay_state_guard = RELAY_STATE.lock().await;
    if let Some(ref mut relay_state) = *relay_state_guard {
        relay_state.stats = RelayStats::default();
        Ok(())
    } else {
        Err(RelayError::NotInitialized)
    }
}

/// Main packet relay task
/// This task listens to the packet channel and handles relay decisions
pub async fn packet_relay_task_impl() {
    info!("Starting packet relay task");

    // Initialize random number generator
    init_random();

    // Initialize the relay state
    {
        let mut relay_state_guard = RELAY_STATE.lock().await;
        *relay_state_guard = Some(RelayState::new());
    }

    // Subscribe to the packet channel
    let mut subscriber = match PACKET_CHANNEL.subscriber() {
        Ok(sub) => sub,
        Err(e) => {
            error!("Failed to subscribe to packet channel: {:?}", e);
            // Consider retry logic or graceful degradation
            return;
        }
    };

    // Main relay loop
    let mut last_stats_log = Instant::now();
    loop {
        // Wait for next packet or timeout for maintenance
        let wait_result = embassy_futures::select::select(
            subscriber.next_message(),
            Timer::after(Duration::from_millis(1000)),
        )
        .await;

        // Process any new packets
        match wait_result {
            embassy_futures::select::Either::First(msg_result) => {
                // Handle received packet
                let packet = match msg_result {
                    embassy_sync::pubsub::WaitResult::Message(packet) => packet,
                    embassy_sync::pubsub::WaitResult::Lagged(_) => {
                        warn!("Packet relay lagged, continuing...");
                        continue;
                    }
                };
                let mut relay_state_guard = RELAY_STATE.lock().await;
                if let Some(ref mut relay_state) = *relay_state_guard {
                    // Log packet receipt immediately to capture all packets
                    let packet_id = packet.header.packet_id;
                    let source = packet.header.source;
                    let destination = packet.header.destination;
                    let port = packet.port_num();
                    let hop_limit = packet.header.flags.hop_limit;

                    info!(
                        "Received packet {} from {:08X} to {:08X} (port: {:?}, hop_limit: {})",
                        packet_id, source, destination, port, hop_limit
                    );

                    // Check for implicit ACKs first
                    relay_state.handle_implicit_ack(&packet);

                    // Decide if we should relay this packet
                    match relay_state.should_relay(&packet) {
                        Ok(true) => {
                            let priority = get_packet_priority(&packet);
                            let wants_ack = packet_wants_ack(&packet);

                            info!(
                                "Queueing packet {} from {:08X} for relay (priority: {}, wants_ack: {})",
                                packet_id, source, priority, wants_ack
                            );

                            if let Err(e) = relay_state.queue_packet(packet, wants_ack, priority) {
                                warn!("Failed to queue packet {}: {:?}", packet_id, e);
                            }
                        }
                        Ok(false) => {
                            // Packet should not be relayed - logging already handled in should_relay()
                            debug!("Packet {} will not be relayed", packet_id);
                        }
                        Err(e) => {
                            warn!("Packet {} validation failed: {:?}", packet_id, e);
                        }
                    }
                }
            }
            embassy_futures::select::Either::Second(_) => {
                // Timeout - perform maintenance
            }
        }

        // Periodically log relay stats (once per minute)
        if last_stats_log.elapsed().as_millis() > 60_000 {
            let relay_state_guard = RELAY_STATE.lock().await;
            if let Some(ref relay_state) = *relay_state_guard {
                let stats = &relay_state.stats;
                info!(
                    "Relay stats: received={}, queued={}, transmitted={}, dropped={}, expired={}, channel_busy={}, duty_cycle_blocks={}, implicit_acks={}, loop_drops={}, retransmissions={}, duty_cycle_util={}%%",
                    stats.packets_received,
                    stats.packets_queued,
                    stats.packets_transmitted,
                    stats.packets_dropped,
                    stats.packets_expired,
                    stats.channel_busy_events,
                    stats.duty_cycle_blocks,
                    stats.implicit_acks,
                    stats.loop_prevention_drops,
                    stats.retransmissions,
                    relay_state.duty_cycle.get_utilization_percent()
                );
            }
            last_stats_log = Instant::now();
        }

        // Handle pending transmissions
        {
            let mut relay_state_guard = RELAY_STATE.lock().await;
            if let Some(ref mut relay_state) = *relay_state_guard {
                // Clean up expired packets
                relay_state.cleanup_expired_packets();
                // Check for packets ready to transmit
                if let Some((packet_id, packet)) = relay_state.get_next_ready_packet() {
                    // Calculate packet airtime for duty cycle checking
                    let packet_airtime_ms = calculate_packet_airtime(&packet); // Check duty cycle limits first
                    if !relay_state.duty_cycle.can_transmit(packet_airtime_ms) {
                        relay_state.stats.duty_cycle_blocks += 1;
                        debug!(
                            "Duty cycle limit would be exceeded for packet {}, delaying",
                            packet_id
                        );
                        // For now, just wait - could implement smarter scheduling
                        continue;
                    }

                    // Check if channel is busy
                    if !is_channel_busy() {
                        info!("Transmitting relay packet {}", packet_id);

                        // Drop the lock before async operations
                        drop(relay_state_guard); // Attempt transmission
                        match transmit_packet(&packet).await {
                            Ok(()) => {
                                // Reacquire lock to update state
                                let mut relay_state_guard = RELAY_STATE.lock().await;
                                if let Some(ref mut relay_state) = *relay_state_guard {
                                    // Record airtime usage
                                    relay_state
                                        .duty_cycle
                                        .record_transmission(packet_airtime_ms);
                                    // Handle successful transmission
                                    relay_state.handle_transmitted_packet(packet_id);
                                } else {
                                    error!("Relay state not initialized during transmission completion");
                                }
                            }
                            Err(e) => {
                                warn!("Failed to transmit packet {}: {}", packet_id, e);
                                // Reacquire lock to handle transmission failure
                                let mut relay_state_guard = RELAY_STATE.lock().await;
                                if let Some(ref mut relay_state) = *relay_state_guard {
                                    // Mark as dropped on transmission failure
                                    relay_state.pending_packets.remove(&packet_id);
                                    relay_state.stats.packets_dropped += 1;
                                }
                            }
                        }
                    } else {
                        debug!(
                            "Channel busy, applying exponential backoff for packet {}",
                            packet_id
                        );
                        relay_state.handle_channel_busy(packet_id);
                    }
                }
            }
        }
    }
}

/// Get comprehensive statistics about the relay system
pub async fn get_relay_stats() -> RelayStats {
    let relay_state_guard = RELAY_STATE.lock().await;
    if let Some(ref relay_state) = *relay_state_guard {
        let mut stats = relay_state.stats.clone();
        // Update current duty cycle percentage
        stats.current_duty_cycle_percent = relay_state.duty_cycle.get_utilization_percent();
        stats
    } else {
        RelayStats::default()
    }
}

/// Manually trigger retransmission of a specific packet (for testing)
pub async fn trigger_retransmission(packet_id: u32) -> bool {
    let mut relay_state_guard = RELAY_STATE.lock().await;
    if let Some(ref mut relay_state) = *relay_state_guard {
        if let Some(pending) = relay_state.pending_packets.get_mut(&packet_id) {
            pending.next_tx_time = Instant::now();
            true
        } else {
            false
        }
    } else {
        false
    }
}
