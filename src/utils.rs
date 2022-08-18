use std::fmt::Write;

use esp_idf_hal::cpu::Core;

mod backtrace;
pub mod executor;
pub mod timer;

pub trait ResultExt<T, E> {
    fn into_error_log(self) -> Option<T>;
}

impl<T, E: std::error::Error> ResultExt<T, E> for Result<T, E> {
    #[track_caller]
    fn into_error_log(self) -> Option<T> {
        let caller = core::panic::Location::caller().file();
        self.map_err(|err| {
            let mut msg = String::new();
            let mut source = err.source();

            if source.is_some() {
                msg = String::with_capacity(64);
                let _ = writeln!(&mut msg);
                let _ = writeln!(&mut msg, "  Caused by:");
            }

            while let Some(err) = &source {
                let _ = writeln!(&mut msg, "  - {err}");
                source = err.source();
            }

            log::error!(target: caller, "{err}{msg}");
        })
        .ok()
    }
}

pub fn set_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        let core = esp_idf_hal::cpu::core();
        println!(
            "\n\n[Core::{}] *** {:#}",
            if core == Core::Core1 {
                "APP(1)"
            } else {
                "PRO(0)"
            },
            panic_info
        );
        println!("\r\nBacktrace:");
        for frame in backtrace::Backtrace::new().take(100) {
            println!("{} ", frame);
        }
        
        loop {}
    }))
}
