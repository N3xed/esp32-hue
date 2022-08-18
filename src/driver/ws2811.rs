use std::iter;

use esp_idf_hal::gpio::OutputPin;
use esp_idf_hal::rmt::config::{Loop, TransmitConfig};
use esp_idf_hal::rmt::{self, PinState};
use esp_idf_hal::units::{Hertz, NanoSeconds};
use esp_idf_sys::{rmt_item32_t, EspError, EOVERFLOW};

/// A `0x00RRGGBB` color value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(transparent)]
pub struct Color(pub u32);

impl From<u32> for Color {
    fn from(val: u32) -> Self {
        Color(val)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ColorGroup {
    /// The amount of consecutive LEDs with the same color.
    pub num_leds: u16,
    /// The color of these LEDs.
    pub color: Color,
}

#[derive(Clone, Default)]
pub struct LedTimings {
    /// The logic `1` high half-period duration.
    pub t0h: NanoSeconds,
    /// The logic `1` low half-period duration.
    pub t0l: NanoSeconds,
    /// The logic `0` high half-period duration.
    pub t1h: NanoSeconds,
    /// The logic `0` low half-period duration.
    pub t1l: NanoSeconds,
}

pub const NEOPIXEL: LedTimings = LedTimings {
    t0h: NanoSeconds(350),
    t0l: NanoSeconds(800),
    t1h: NanoSeconds(750),
    t1l: NanoSeconds(600),
};

pub const WS2811_HS: LedTimings = LedTimings {
    t0h: NanoSeconds(300),
    t0l: NanoSeconds(1000),
    t1h: NanoSeconds(700),
    t1l: NanoSeconds(600),
};

#[repr(transparent)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
struct RmtItem(u32);

impl RmtItem {
    pub const fn new(duration0: u16, level0: bool, duration1: u16, level1: bool) -> RmtItem {
        let half_period0: u32 = (duration0 as u32) | ((level0 as u32) << 15);
        let half_period1: u32 = (duration1 as u32) | ((level1 as u32) << 15);

        RmtItem(half_period0 | (half_period1 << 16))
    }

    pub const fn into_rmt_item32_t(self) -> rmt_item32_t {
        unsafe { std::mem::transmute(self) }
    }
}

pub struct Ws2811<PIN: OutputPin> {
    rmt: rmt::Transmit<PIN, rmt::CHANNEL0>,
    zero_item: rmt_item32_t,
    one_item: rmt_item32_t,
}

impl<PIN: OutputPin> Ws2811<PIN> {
    pub fn new(pin: PIN, channel: rmt::CHANNEL0) -> Result<Self, EspError> {
        let cfg = TransmitConfig {
            clock_divider: 4,
            mem_block_num: 8,
            carrier: None,
            looping: Loop::None,
            idle: Some(PinState::Low),
            aware_dfs: false,
        };
        let rmt = rmt::Transmit::new(pin, channel, &cfg)?;

        let mut result = Ws2811 {
            rmt,
            zero_item: Default::default(),
            one_item: Default::default(),
        };
        result.set_led_timings(&NEOPIXEL)?;

        Ok(result)
    }

    pub fn set_led_timings(&mut self, timings: &LedTimings) -> Result<(), EspError> {
        let clock_hz = self.rmt.counter_clock()?;

        self.zero_item = RmtItem::new(
            nanos_to_ticks(clock_hz, timings.t0h)?,
            true,
            nanos_to_ticks(clock_hz, timings.t0l)?,
            false,
        )
        .into_rmt_item32_t();
        self.one_item = RmtItem::new(
            nanos_to_ticks(clock_hz, timings.t1h)?,
            true,
            nanos_to_ticks(clock_hz, timings.t1l)?,
            false,
        )
        .into_rmt_item32_t();

        Ok(())
    }

    pub fn show<I>(&mut self, iter: I) -> Result<(), EspError>
    where
        I: Iterator<Item = ColorGroup> + Send,
    {
        let zero_item = self.zero_item;
        let one_item = self.one_item;
        let iter = iter
            .flat_map(|g| iter::repeat(g.color.0).take(g.num_leds as usize))
            .flat_map(|val| {
                let mut mask = 1 << 24;
                (0_u32..24).map(move |_| {
                    mask >>= 1;
                    if (val & mask) == 0 {
                        zero_item
                    } else {
                        one_item
                    }
                })
            });

        self.rmt.start_iter_blocking(iter)
    }
}

fn nanos_to_ticks(ticks_hz: Hertz, duration: NanoSeconds) -> Result<u16, EspError> {
    const NANOSECONDS_PER_SECOND: u32 = 1_000_000_000;
    const BITS15_MASK: u32 = 0x7fff;

    (ticks_hz.0 as u128)
        .checked_mul(duration.0 as u128)
        // round to nearest digit
        .and_then(|v| v.checked_add((NANOSECONDS_PER_SECOND / 2) as u128))
        .and_then(|v| v.checked_div(NANOSECONDS_PER_SECOND as u128))
        .and_then(|v| {
            if v & !(BITS15_MASK as u128) == 0 {
                Some(v as u16)
            } else {
                None
            }
        })
        .ok_or(EspError::from(EOVERFLOW as i32).unwrap())
}
