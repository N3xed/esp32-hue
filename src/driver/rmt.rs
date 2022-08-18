use std::borrow::Borrow;

use esp_idf_hal::gpio;
use esp_idf_sys as sys;
use sys::{esp_nofail, esp_result, EspError};

/// The size of a RMT memory block in [`RmtItem`]s.
pub const RMT_MEM_BLOCK_SIZE: usize = 64;
/// Amount of rmt channels (equals amount of rmt memory blocks)
pub const CHANNEL_COUNT: usize = 8;

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Low level
    Low = 0,
    /// High level
    High,
}

pub struct TxConfig {
    pub carrier_freq_hz: u32,
    pub carrier_level: Level,
    pub idle_level: Level,
    pub carrier_duty_percent: u8,
    pub carrier_en: bool,
    pub loop_en: bool,
    pub idle_output_en: bool,
}

pub struct RxConfig {
    pub idle_threshold: u16,
    pub filter_ticks_thresh: u8,
    pub filter_en: bool,
}

pub enum Mode {
    Tx(TxConfig),
    Rx(RxConfig),
}

impl Into<sys::rmt_mode_t> for &Mode {
    fn into(self) -> sys::rmt_mode_t {
        match self {
            Mode::Tx(_) => sys::rmt_mode_t_RMT_MODE_TX,
            Mode::Rx(_) => sys::rmt_mode_t_RMT_MODE_RX,
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ClockSource {
    APB = 0,
    Ref = sys::RMT_CHANNEL_FLAGS_AWARE_DFS as _,
}

pub struct Config {
    pub mode: Mode,
    pub clk_div: u8,
    pub mem_block_count: u8,
    pub clock_src: ClockSource,
    pub always_on: bool,
}

pub struct Rmt<PIN: gpio::Pin> {
    _pin: PIN,
    channel: sys::rmt_channel_t,
}

impl<PIN: gpio::Pin> Rmt<PIN> {
    /// Create the driver to the remote peripheral with the `register` and `pin`
    pub fn new(pin: PIN, channel: sys::rmt_channel_t) -> Rmt<PIN> {
        Rmt { _pin: pin, channel }
    }

    /// Configure the remote peripheral using `config`
    pub fn configure(&mut self, config: Config) -> Result<(), EspError> {
        let ll_cfg = sys::rmt_config_t {
            rmt_mode: config.mode.borrow().into(),
            channel: self.channel,
            gpio_num: PIN::pin(),
            clk_div: config.clk_div,
            mem_block_num: config.mem_block_count,
            flags: config.clock_src as u32
                | if config.always_on {
                    sys::RMT_CHANNEL_FLAGS_ALWAYS_ON
                } else {
                    0
                },
            __bindgen_anon_1: match config.mode {
                Mode::Tx(tx_cfg) => {
                    let cfg = sys::rmt_tx_config_t {
                        carrier_duty_percent: tx_cfg.carrier_duty_percent,
                        carrier_en: tx_cfg.carrier_en as _,
                        carrier_freq_hz: tx_cfg.carrier_freq_hz,
                        carrier_level: tx_cfg.carrier_level as _,
                        idle_level: tx_cfg.idle_level as _,
                        idle_output_en: tx_cfg.idle_output_en as _,
                        loop_en: tx_cfg.loop_en as _,
                    };
                    sys::rmt_config_t__bindgen_ty_1 { tx_config: cfg }
                }
                Mode::Rx(rx_cfg) => {
                    let cfg = sys::rmt_rx_config_t {
                        filter_en: rx_cfg.filter_en,
                        filter_ticks_thresh: rx_cfg.filter_ticks_thresh,
                        idle_threshold: rx_cfg.idle_threshold,
                    };
                    sys::rmt_config_t__bindgen_ty_1 { rx_config: cfg }
                }
            },
        };

        unsafe {
            esp_result!(sys::rmt_config(&ll_cfg as _), ())?;
            esp_result!(sys::rmt_driver_install(self.channel, 0, 0), ())?;
        }

        Ok(())
    }

    /// Write all items in `iter` using the remote peripheral and depending on `wait_done`
    /// wait until all items were sent.
    pub fn write<T>(&self, iter: T, wait_done: bool)
    where
        T: Iterator<Item = RmtItem> + Send + 'static,
    {
        unsafe {
            let iter = Box::new(iter);

            esp_nofail!(sys::rmt_translator_init(
                self.channel,
                Some(tx_translate_iterator::<T>),
            ));

            esp_nofail!(sys::rmt_write_sample(
                self.channel,
                Box::leak(iter) as *mut _ as _,
                1,
                wait_done
            ));
        }
    }
}

unsafe extern "C" fn tx_translate_iterator<T>(
    src: *const sys::c_types::c_void,
    dest: *mut sys::rmt_item32_t,
    src_size: u32,
    wanted_num: u32,
    translated_size: *mut u32,
    item_num: *mut u32,
) where
    T: Iterator<Item = RmtItem> + Send + 'static,
{
    let iter = src as *mut T;
    let dest = std::slice::from_raw_parts_mut(dest as *mut u32, wanted_num as usize);

    let mut i = 0;
    let finished = loop {
        if i >= wanted_num {
            break 0;
        }

        if let Some(item) = (&mut *iter).next() {
            dest[i as usize] = item.0;
            i += 1;
        } else {
            // deallocate the iterator
            drop(Box::from_raw(iter));
            break src_size;
        }
    };

    *item_num = i;
    *translated_size = finished;
}

#[repr(transparent)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct RmtItem(pub u32);

impl RmtItem {
    pub const fn new(duration0: u16, level0: bool, duration1: u16, level1: bool) -> RmtItem {
        let half_period0: u32 = (duration0 as u32) | ((level0 as u32) << 15);
        let half_period1: u32 = (duration1 as u32) | ((level1 as u32) << 15);

        RmtItem(half_period0 | (half_period1 << 16))
    }

    pub const fn duration0(self) -> u16 {
        (self.0 & 0b01111_1111_1111_1111) as _
    }

    pub const fn level0(self) -> bool {
        (self.0 & 0b1000_0000_0000_0000) != 0
    }

    pub const fn duration1(self) -> u16 {
        ((self.0 >> 16) & 0b01111_1111_1111_1111) as _
    }

    pub const fn level1(self) -> bool {
        (self.0 & 0b1000_0000_0000_0000_0000_0000_0000_0000) != 0
    }
}
