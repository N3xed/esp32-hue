use core::future::Future;
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::Waker;
use std::sync::Arc;

use esp_idf_hal::interrupt;

pub struct Executor {
    state: spin::RwLock<ExecutorState>,
}

pub struct ExecutorState {
    enqueue_task: Option<&'static (dyn Fn(TaskId) + Send + Sync)>,
}

impl Executor {
    pub const fn new() -> Self {
        Executor {
            state: spin::RwLock::new(ExecutorState { enqueue_task: None }),
        }
    }

    pub fn run<const N: usize>(
        &'static self,
        tasks: &mut [&mut (dyn Future<Output = ()> + Unpin)],
    ) {
        let queue = heapless::mpmc::MpMcQueue::<TaskId, N>::new();
        let wakers: heapless::Vec<Waker, N> = tasks
            .iter()
            .enumerate()
            .map(|(id, _)| {
                queue.enqueue(id as TaskId).expect("too many tasks");
                TaskHandle::new(self, id as TaskId).into_waker()
            })
            .collect();
        let tasks_enqueued: [AtomicBool; N] = {
            const VAL: AtomicBool = AtomicBool::new(false);
            [VAL; N]
        };

        let thread_handle =
            NonNull::new(interrupt::task::current().expect("in interrupt")).unwrap();

        let enqueue_task = |task_id: TaskId| {
            // Only enqueue the task once.
            if let Ok(_) = tasks_enqueued[task_id].compare_exchange(
                false,
                true,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                let _ = queue.enqueue(task_id);
                unsafe {
                    interrupt::task::notify_one(thread_handle.as_ptr());
                }
                unsafe {
                    esp_idf_sys::esp_rom_printf(b"!\0" as *const _ as *const i8);
                }
            }
        };

        {
            let mut state = self.state.write();
            // Safe because its behind a mutex and we set it to `None` at the end of the function.
            state.enqueue_task = unsafe { std::mem::transmute(&enqueue_task as &dyn Fn(TaskId)) };
        }

        let mut pending_futures = tasks.len();
        unsafe {
            interrupt::task::notify(thread_handle.as_ptr(), pending_futures as u32);
        }

        while pending_futures > 0 {
            interrupt::task::wait_one_notification();
            if let Some(task_id) = queue.dequeue() {
                tasks_enqueued[task_id].store(false, Ordering::SeqCst);

                let waker = &wakers[task_id];
                let mut context = core::task::Context::from_waker(waker);
                let fut = &mut *tasks[task_id];

                if Pin::new(fut).poll(&mut context).is_ready() {
                    pending_futures -= 1;
                }
            }
        }

        {
            let mut state = self.state.write();
            state.enqueue_task = None;
        }
    }
}

#[derive(Clone)]
pub struct TaskHandle(Arc<TaskHandleData>);

type TaskId = usize;

struct TaskHandleData {
    executor: &'static Executor,
    id: TaskId,
}

impl TaskHandleData {
    fn enqueue_task(&self) {
        let executor_data = self.executor.state.read();

        match executor_data.enqueue_task {
            Some(enqueue_task) => {
                (*enqueue_task)(self.id);
            }
            _ => {
                log::debug!("cannot enqueue task: executor is dead")
            }
        }
    }

    unsafe fn into_raw_waker(data: *const TaskHandleData) -> core::task::RawWaker {
        core::task::RawWaker::new(
            data as *const (),
            &core::task::RawWakerVTable::new(
                TaskHandle::waker_clone,
                TaskHandle::waker_wake,
                TaskHandle::waker_wake_by_ref,
                TaskHandle::waker_drop,
            ),
        )
    }
}

impl TaskHandle {
    pub fn new(executor: &'static Executor, id: usize) -> Self {
        Self(Arc::new(TaskHandleData { executor, id }))
    }

    pub fn into_waker(self) -> Waker {
        let arc_data = Arc::into_raw(self.0);
        unsafe { Waker::from_raw(TaskHandleData::into_raw_waker(arc_data)) }
    }

    unsafe fn waker_clone(arc_data: *const ()) -> core::task::RawWaker {
        let arc_data = arc_data as *const TaskHandleData;
        Arc::increment_strong_count(arc_data);
        TaskHandleData::into_raw_waker(arc_data)
    }
    unsafe fn waker_wake(arc_data: *const ()) {
        let arc_data = Arc::from_raw(arc_data as *const TaskHandleData);
        arc_data.enqueue_task();
    }
    unsafe fn waker_wake_by_ref(arc_data: *const ()) {
        let arc_data = &*(arc_data as *const TaskHandleData);
        arc_data.enqueue_task();
    }
    unsafe fn waker_drop(arc_data: *const ()) {
        drop(Arc::from_raw(arc_data as *const TaskHandleData));
    }
}
