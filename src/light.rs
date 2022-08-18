use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use embedded_svc::channel::asynch::Receiver;
// use embedded_svc::executor::asynch::{Executor, WaitableExecutor};
use embedded_svc::timer::asynch::{OnceTimer, PeriodicTimer};
use esp_idf_hal::gpio::OutputPin;
use esp_idf_hal::units::{FromValueType, MicroSecondsU64};
use esp_idf_hal::{self, rmt};
// use esp_idf_svc::executor::asynch::isr::tasks_spawner;
use esp_idf_sys::EspError;
use futures::channel::mpsc::{self, channel, Sender};
use futures::{pin_mut, select, FutureExt, StreamExt};
use heapless::mpmc::MpMcQueue;
use palette::convert::IntoColorUnclamped;
use palette::rgb::Rgb;
use palette::Packed;

use crate::driver::ws2811::{Color, ColorGroup, Ws2811};
use crate::utils::executor::Executor;
use crate::utils::timer::EspTimer;

#[derive(Debug, thiserror::Error)]
#[error("failed to start light service")]
pub struct StartError(#[from] InitError);

#[derive(Debug, thiserror::Error)]
pub enum InitError {
    #[error("failed to initialize rmt peripheral")]
    Rmt(#[source] EspError),
}

pub enum Message {}

pub type MessageSender = Sender<Message>;

pub fn start(
    pin: impl OutputPin + 'static,
    rmt_channel: rmt::CHANNEL0,
) -> Result<MessageSender, StartError> where
{
    let (sender, receiver) = channel(2);

    let ws2811 = Ws2811::new(pin, rmt_channel).map_err(InitError::Rmt)?;
    let timer = EspTimer::new();

    static EXECUTOR: Executor = Executor::new();

    std::thread::spawn(move || {
        let task = run(ws2811, receiver, timer);
        pin_mut!(task);
        EXECUTOR.run::<2>(&mut [&mut task]);

        log::info!("light service shut down");
    });

    Ok(sender)
}

async fn run<P: OutputPin>(
    mut ws2811: Ws2811<P>,
    mut msg_recv: mpsc::Receiver<Message>,
    mut timer: EspTimer,
) {
    let mut col = palette::Hsv::<_, f32>::new(0., 1.0, 1.0);
    let offset = 360_f32 / (5_f32 * 60_f32);
    let mut color_group = ColorGroup {
        color: Color(0),
        num_leds: 10,
    };

    loop {
        let rgb_col: Rgb = col.into_color_unclamped();
        let rgb_col: Rgb<_, u8> = rgb_col.into_format();
        let rgb_col: Packed = rgb_col.into();
        color_group.color = rgb_col.color.into();
        col.hue += offset;

        ws2811.show(std::iter::once(color_group)).unwrap();

        let sleep = timer.after(MicroSecondsU64(16000)).unwrap();
        sleep.await;

        // let msg = select! {
        //     () = sleep => continue,
        //     msg = msg_recv.next() => match msg {
        //         None => {
        //             panic!("got None");
        //             SHOULD_QUIT.store(true, Ordering::Relaxed);
        //             break;
        //         },
        //         Some(msg) => msg
        //     }
        // };

        // match msg {}
    }
}
