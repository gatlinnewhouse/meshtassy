//! Example board template - copy this file and modify for your board
//!
//! Pin assignments and peripheral configuration for [YOUR BOARD NAME].

use super::{BoardPeripherals, LoRaPeripherals};
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::{bind_interrupts, i2c, peripherals, spi, usb};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use static_cell::StaticCell;

bind_interrupts!(struct Irqs {
    I2C0_IRQ => i2c::InterruptHandler<peripherals::I2C0>;
    USBCTRL_IRQ => usb::InterruptHandler<peripherals::USB>;
});

/// Pin assignments for [YOUR BOARD NAME]
///
/// TODO: Document your board's pin assignments here
///
/// LoRa Radio (ANY HAT USED?):
/// - NSS (Chip Select): PIN_X
/// - Reset: PIN_X
/// - DIO1: PIN_X
/// - Busy: PIN_X
/// - SPI: SPIX (SCK: PIN_X, MISO: PIN_X, MOSI: PIN_X, TX_DMA: DMA_CHX, RX_DMA: DMA_CHX)
/// - I2C: (I2CX SCL: PIN_X, SDA: PIN_X)
///
/// This board has no user-controllable LEDs
pub fn init_board(p: embassy_rp::Peripherals) -> BoardPeripherals {
    // Configure LoRa radio pins (replace with actual pins for your board)
    // TODO: Update these pin assignments for your board
    let nss = Output::new(p.PIN_3, Level::High);
    let reset = Output::new(p.PIN_15, Level::High);
    let dio1 = Input::new(p.PIN_20, Pull::Down);
    let busy = Input::new(p.PIN_2, Pull::None);

    // Configure SPI for LoRa radio (replace with actual pins for your board)
    // TODO: Update these pin assignments for your board
    let spi_config = spi::Config::default();
    let spi_sck = p.PIN_10;
    let spi_miso = p.PIN_12;
    let spi_mosi = p.PIN_11;
    let spi_tx_dma = p.DMA_CH0;
    let spi_rx_dma = p.DMA_CH1;
    let spi_dev = spi::Spi::new(
        p.SPI1, spi_sck, spi_mosi, spi_miso, spi_tx_dma, spi_rx_dma, spi_config,
    );
    let spi = ExclusiveDevice::new(spi_dev, nss, Delay);

    // Configure USB driver
    let usb_driver = usb::Driver::new(p.USB, Irqs);

    // Configure RNG
    //TODO: investigate actual solution as this amy be wrong
    let rng = embassy_rp::clocks::RoscRng;

    // I2C bus config
    // TODO: Update these pin assignments for your board
    let i2c_config = i2c::Config::default();
    let i2c_scl = p.PIN_5;
    let i2c_sda = p.PIN_4;
    static I2C_BUS: StaticCell<
        Mutex<CriticalSectionRawMutex, i2c::I2c<'static, peripherals::I2C0, i2c::Async>>,
    > = StaticCell::new();
    let i2c = i2c::I2c::new_async(p.I2C0, i2c_scl, i2c_sda, Irqs, i2c_config);
    let i2c_bus = I2C_BUS.init(Mutex::new(i2c));

    BoardPeripherals {
        lora: LoRaPeripherals {
            spi,
            reset,
            dio1,
            busy,
        },
        leds: None, // This board has no LEDs
        usb_driver,
        rng,
        i2c: Some(i2c_bus),
    }
}
