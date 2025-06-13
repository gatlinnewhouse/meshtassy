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

/// Maximum time to keep a packet before giving up (60 seconds)
const MAX_PACKET_AGE_MS: u64 = 60_000;

/// Our node ID (hardcoded for now, should come from config)
const OUR_NODE_ID: u32 = 0xDEADBEEF;

/// Channel utilization limit (10%)
const CHANNEL_UTIL_LIMIT: u8 = 10;

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
    recent_packets: Vec<u32, 32>,
    /// Our random node ID for this session
    our_node_id: u32,
    /// Whether RX boost is enabled (affects timing)
    rx_boost_enabled: bool,
}

impl RelayState {
    fn new() -> Self {
        Self {
            pending_packets: FnvIndexMap::new(),
            recent_packets: Vec::new(),
            our_node_id: OUR_NODE_ID,
            rx_boost_enabled: true, // Assume boosted for conservative timing
        }
    }

    /// Check if we've recently seen this packet (loop prevention)
    fn is_recent_packet(&self, packet_id: u32) -> bool {
        self.recent_packets.contains(&packet_id)
    }

    /// Add a packet to the recent packets list
    fn add_recent_packet(&mut self, packet_id: u32) {
        if self.recent_packets.len() >= 32 {
            self.recent_packets.remove(0);
        }
        let _ = self.recent_packets.push(packet_id);
    }

    /// Check if a packet should be relayed
    fn should_relay(&self, packet: &DecodedPacket) -> bool {
        // Don't relay our own packets
        if packet.header.source == self.our_node_id {
            return false;
        }

        // Don't relay if we've seen this packet recently
        if self.is_recent_packet(packet.header.packet_id) {
            return false;
        }

        // Check hop limit
        if packet.header.flags.hop_limit == 0 {
            return false;
        }

        // Don't relay direct messages not meant for us
        if packet.header.destination != 0xFFFFFFFF && packet.header.destination != self.our_node_id {
            // This is a directed packet not for us
            // In a real implementation, we'd check if we're the intended next hop
            return false;
        }

        // Check port - some ports shouldn't be relayed
        match packet.port_num() {
            femtopb::EnumValue::Known(PortNum::RoutingApp) => true,
            femtopb::EnumValue::Known(PortNum::TextMessageApp) => true,
            femtopb::EnumValue::Known(PortNum::NodeinfoApp) => true,
            femtopb::EnumValue::Known(PortNum::PositionApp) => true,
            femtopb::EnumValue::Known(PortNum::TelemetryApp) => true,
            _ => false, // Be conservative with unknown ports
        }
    }    /// Calculate the initial relay delay for a packet
    fn calculate_relay_delay(&self, packet: &DecodedPacket, priority: u8) -> u64 {
        // Base delay depends on RX boost setting
        let base_delay = if self.rx_boost_enabled {
            MAX_HOP_DELAY_MS
        } else {
            MIN_HOP_DELAY_MS
        };

        // Add penalty for each hop already taken
        let hops_taken = packet.header.flags.hop_start.saturating_sub(packet.header.flags.hop_limit);
        let hop_penalty = hops_taken as u64 * HOP_PENALTY_MS;        // Apply priority multiplier
        let adjusted_delay = ((base_delay + hop_penalty) as f32 * priority_delay_multiplier(priority)) as u64;// Add random jitter (0 to 2x adjusted delay)
        // Use a simple PRNG based on current time since fastrand isn't available
        let time_seed = embassy_time::Instant::now().as_millis() as u64;
        let jitter = (time_seed * 1103515245 + 12345) % (2 * adjusted_delay + 1);
        
        adjusted_delay + jitter
    }    /// Add a packet for potential relay
    fn queue_packet(&mut self, packet: DecodedPacket, wants_ack: bool, priority: u8) {
        let now = Instant::now();
        let delay_ms = self.calculate_relay_delay(&packet, priority);
        let max_retransmissions = get_max_retransmissions(&packet, wants_ack);
        
        let pending = PendingPacket {
            packet: packet.clone(),
            queued_at: now,
            next_tx_time: now + Duration::from_millis(delay_ms),
            max_retransmissions,
            retry_count: 0,
            wants_ack,
            priority,
        };

        // Store in pending packets map
        let packet_id = packet.header.packet_id;
        if self.pending_packets.insert(packet_id, pending).is_err() {
            warn!("Failed to queue packet {} - pending queue full", packet_id);
        } else {
            debug!("Queued packet {} for relay in {}ms (max retries: {})", packet_id, delay_ms, max_retransmissions);
        }

        // Mark as recently seen
        self.add_recent_packet(packet_id);
    }/// Handle implicit ACK (when we hear our broadcast echoed)
    fn handle_implicit_ack(&mut self, packet: &DecodedPacket) {
        // If we hear someone else rebroadcasting our packet, consider it acknowledged
        if packet.header.source == self.our_node_id {
            if self.pending_packets.contains_key(&packet.header.packet_id) {
                info!("Implicit ACK received for packet {}", packet.header.packet_id);
                self.pending_packets.remove(&packet.header.packet_id);
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
    }    /// Handle a packet that was just transmitted
    fn handle_transmitted_packet(&mut self, packet_id: u32) {
        if let Some(pending) = self.pending_packets.get_mut(&packet_id) {
            if pending.has_retransmissions_left() {
                // Schedule next retransmission
                pending.retry_count += 1;
                
                let retry_interval = if (pending.retry_count as usize) <= RETX_INTERVALS.len() {
                    RETX_INTERVALS[pending.retry_count as usize - 1]
                } else {
                    RETX_INTERVALS[RETX_INTERVALS.len() - 1]
                };
                
                pending.next_tx_time = Instant::now() + Duration::from_millis(retry_interval);
                
                debug!(
                    "Scheduled retransmission {} for packet {} in {}ms (retries left: {})", 
                    pending.retry_count, packet_id, retry_interval, pending.retransmissions_left()
                );
            } else {
                // No more retransmissions
                self.pending_packets.remove(&packet_id);
                info!("Packet {} transmission complete", packet_id);
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
    matches!(packet.port_num(), femtopb::EnumValue::Known(PortNum::RoutingApp))
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
    let mut subscriber = PACKET_CHANNEL.subscriber().unwrap();
    
    // Main relay loop
    loop {        // Wait for next packet or timeout for maintenance
        let wait_result = embassy_futures::select::select(
            subscriber.next_message(),
            Timer::after(Duration::from_millis(1000))
        ).await;
        
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
                    // Check for implicit ACKs first
                    relay_state.handle_implicit_ack(&packet);
                    
                    // Decide if we should relay this packet
                    if relay_state.should_relay(&packet) {
                        let priority = get_packet_priority(&packet);
                        let wants_ack = packet_wants_ack(&packet);
                        
                        info!(
                            "Queueing packet {} from {:08X} for relay", 
                            packet.header.packet_id, packet.header.source
                        );
                        
                        relay_state.queue_packet(packet, wants_ack, priority);
                    }
                }
            }
            embassy_futures::select::Either::Second(_) => {
                // Timeout - perform maintenance
            }
        }
        
        // Handle pending transmissions
        {
            let mut relay_state_guard = RELAY_STATE.lock().await;
            if let Some(ref mut relay_state) = *relay_state_guard {
                // Clean up expired packets
                relay_state.cleanup_expired_packets();
                
                // Check for packets ready to transmit
                if let Some((packet_id, packet)) = relay_state.get_next_ready_packet() {
                    // Check if channel is busy
                    if !is_channel_busy() {
                        info!("Transmitting relay packet {}", packet_id);
                        
                        // Drop the lock before async operations
                        drop(relay_state_guard);
                        
                        // Attempt transmission
                        match transmit_packet(&packet).await {
                            Ok(()) => {
                                // Reacquire lock to update state
                                let mut relay_state_guard = RELAY_STATE.lock().await;
                                if let Some(ref mut relay_state) = *relay_state_guard {
                                    relay_state.handle_transmitted_packet(packet_id);
                                }
                            }
                            Err(e) => {
                                warn!("Failed to transmit packet {}: {}", packet_id, e);
                            }
                        }
                    } else {
                        debug!("Channel busy, delaying transmission of packet {}", packet_id);
                        // TODO: Implement exponential backoff for channel busy conditions
                    }
                }
            }
        }
    }
}

/// Get statistics about the relay system
pub async fn get_relay_stats() -> (usize, usize) {
    let relay_state_guard = RELAY_STATE.lock().await;
    if let Some(ref relay_state) = *relay_state_guard {
        (relay_state.pending_packets.len(), relay_state.recent_packets.len())
    } else {
        (0, 0)
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
