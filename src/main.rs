#![feature(generic_associated_types)]

use std::sync::Arc;
use std::time::Duration;

use embedded_svc::timer::asynch::TimerService;
use embedded_svc::utils::asyncify::timer::AsyncTimerService;
use embedded_svc::utils::asyncify::Asyncify;
use embedded_svc::wifi::{self, Wifi};
use esp_idf_hal::prelude::Peripherals;
use esp_idf_svc::netif::EspNetifStack;
use esp_idf_svc::nvs::EspDefaultNvs;
use esp_idf_svc::sysloop::EspSysLoopStack;
use esp_idf_svc::timer::{EspISRTimerService, EspTaskTimerService};
use esp_idf_svc::wifi::EspWifi;
use esp_idf_sys as _;

use crate::utils::ResultExt;

mod driver;
mod hue;
mod light;
mod utils;

fn main() {
    esp_idf_sys::link_patches();
    utils::set_panic_hook();
    esp_idf_svc::log::EspLogger::initialize_default();

    log::info!("Starting...");

    let peripherals = Peripherals::take().unwrap();

    let nvs = Arc::new(EspDefaultNvs::new().expect("failed to create nvs"));
    // let mut timers: AsyncTimerService<EspTaskTimerService, _> =
    //     EspTaskTimerService::new().unwrap().into_async();

    let light_channel = light::start(
        peripherals.pins.gpio5.into_output().unwrap(),
        peripherals.rmt.channel0,
    )
    .into_error_log();

    let netif = Arc::new(EspNetifStack::new().expect("failed to create netif"));
    let sysloop = Arc::new(EspSysLoopStack::new().expect("failed to create sysloop"));
    let mut wifi = EspWifi::new(netif, sysloop, nvs.clone()).expect("could not initialize WiFi");

    wifi.set_configuration(&wifi::Configuration::Client(wifi::ClientConfiguration {
        ssid: "Wokwi-GUEST".into(),
        password: "".into(),
        channel: Some(6),
        auth_method: wifi::AuthMethod::None,
        ..Default::default()
    }))
    .expect("failed to set wifi config");

    loop {
        std::thread::sleep(Duration::from_millis(100));
    }
}
