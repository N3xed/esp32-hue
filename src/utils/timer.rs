use core::future::Future;
use std::ptr::NonNull;

use esp_idf_hal::units::MicroSecondsU64;
use esp_idf_sys as sys;
use sys::c_types::c_void;
use sys::{esp, esp_nofail, esp_timer_create, EspError};

type EspTimerHandle = Option<NonNull<sys::esp_timer>>;

#[derive(Default)]
pub struct EspTimer {
    handle: EspTimerHandle,
    waker: Option<core::task::Waker>,
}

unsafe impl Send for EspTimer {}

impl EspTimer {
    pub const fn new() -> Self {
        EspTimer {
            handle: None,
            waker: None,
        }
    }

    fn init(&mut self) -> Result<(), EspError> {
        #[cfg(esp_idf_esp_timer_supports_isr_dispatch_method)]
        let dispatch_method = sys::esp_timer_dispatch_t_ESP_TIMER_ISR;
        #[cfg(not(esp_idf_esp_timer_supports_isr_dispatch_method))]
        let dispatch_method = esp_timer_dispatch_t_ESP_TIMER_TASK;

        unsafe {
            esp!(esp_timer_create(
                &sys::esp_timer_create_args_t {
                    callback: Some(Self::handle_callback),
                    name: b"EspTimer\0" as *const _ as *const _, // TODO
                    arg: self as *mut _ as *mut _,
                    dispatch_method,
                    skip_unhandled_events: false, // TODO
                },
                std::mem::transmute(&mut self.handle),
            ))
        }
    }

    pub fn after<'a>(
        &'a mut self,
        timeout: MicroSecondsU64,
    ) -> Result<impl Future<Output = ()> + 'a, EspError> {
        if let None = &self.handle {
            self.init()?;
        }

        Ok(futures::future::poll_fn(move |ctx| {
            if let None = &self.waker {
                self.waker = Some(ctx.waker().clone());
                unsafe {
                    esp_nofail!(sys::esp_timer_start_once(
                        self.handle.unwrap().as_ptr(),
                        timeout.0
                    ));
                }

                core::task::Poll::Pending
            } else {
                self.waker = None;
                core::task::Poll::Ready(())
            }
        }))
    }

    extern "C" fn handle_callback(arg: *mut c_void) {
        let this = unsafe { &mut *(arg as *mut EspTimer) };
        if let Some(waker) = &this.waker {
            waker.wake_by_ref();
        }

        #[cfg(esp_idf_esp_timer_supports_isr_dispatch_method)]
        unsafe {
            sys::esp_timer_isr_dispatch_need_yield();
        }
    }
}
