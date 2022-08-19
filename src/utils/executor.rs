use core::future::Future;
use core::mem::ManuallyDrop;
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{self, Waker};
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;

use esp_idf_hal::interrupt;
use heapless::{spsc, Vec};

/// A minimal executor.
pub struct Executor {
    state: spin::Mutex<ExecutorState>,
}

unsafe impl Sync for Executor {}

pub struct ExecutorState {
    enqueue_task: Option<NonNull<(dyn FnMut(TaskId) + Send)>>,
}

impl ExecutorState {
    #[inline(always)]
    fn enqueue_task(&mut self, task_id: TaskId) {
        match self.enqueue_task {
            Some(mut enqueue_task) => unsafe { (enqueue_task.as_mut())(task_id) },
            None => log::debug!("cannot enqueue task: executor is dead"),
        }
    }
}

impl Executor {
    /// Create a new [`Executor`], the executor must live forever to be useful.
    pub const fn new() -> Self {
        Executor {
            state: spin::Mutex::new(ExecutorState { enqueue_task: None }),
        }
    }

    /// Run the exeuctor with the given `tasks`.
    ///
    /// TODO: document behavior
    pub fn run<const N: usize>(
        &'static self,
        tasks: &mut [&mut (dyn Future<Output = ()> + Unpin)],
    ) {
        let mut queue = spsc::Queue::<TaskId, N>::new();
        let (mut send, mut receive) = queue.split();
        let task_handles: Vec<TaskHandle, N> = tasks
            .iter()
            .enumerate()
            .map(|(id, _)| {
                send.enqueue(id as TaskId).expect("task queue full");
                TaskHandle::new(self, id as TaskId)
            })
            .collect();

        let thread_handle =
            NonNull::new(interrupt::task::current().expect("in interrupt")).unwrap();

        let mut enqueue_task = {
            let thread_handle = thread_handle.clone();
            move |task_id: TaskId| {
                send.enqueue(task_id).expect("task queue full");
                unsafe {
                    interrupt::task::notify(thread_handle.as_ptr(), 1);
                }
            }
        };

        {
            let mut state = self.state.lock();
            // Safe to share with other threads since we make sure that `enqueue_task`
            // isn't called again when `tasks_enqueued` goes out-of-scope, the thread
            // associated with `thread_handle` doesn't exist anymore, and `enqueue_task`
            // is called uniquely (only one thread at a time).
            //
            // This is done by having anyone wanting to call this closure acquire the
            // `Executor::state` mutex lock, and at the end of this function we acquire the
            // mutex lock and set the closure reference to `None`.
            state.enqueue_task =
                unsafe { std::mem::transmute(&mut enqueue_task as &mut (dyn FnMut(TaskId))) };
        }

        unsafe {
            interrupt::task::notify(thread_handle.as_ptr(), 1);
        }

        let mut pending_futures = tasks.len();
        while pending_futures > 0 {
            interrupt::task::wait_notification(None);

            while let Some(task_id) = receive.dequeue() {
                let handle = &task_handles[task_id];
                handle.0.is_queued.store(false, Ordering::Relaxed);

                let waker = handle.as_waker();
                let mut context = task::Context::from_waker(&waker);
                let fut = &mut *tasks[task_id];

                if Pin::new(fut).poll(&mut context).is_ready() {
                    pending_futures -= 1;
                }
            }
        }

        {
            let mut state = self.state.lock();
            state.enqueue_task = None;
        }
    }
}

/// A handle to a task given to [`Executor::run`].
#[derive(Clone)]
struct TaskHandle(Arc<TaskHandleData>);

type TaskId = usize;

struct TaskHandleData {
    executor: &'static Executor,
    id: TaskId,
    is_queued: AtomicBool,
}

impl TaskHandleData {
    #[inline]
    fn enqueue_task(&self) {
        // Only enqueue the task once.
        //
        // This field gets reset by [`Executor::run`] once the task has been dequeued.
        // Having this field here also means that the `Arc<TaskHandleData>` must be unique
        // per task, which is fufilled by only letting [`Executor::run`] give out
        // [`TaskHandle`]s ([`TaskHandle::new`] must be private).
        if let Ok(_) =
            self.is_queued
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        {
            let mut executor_data = self.executor.state.lock();
            executor_data.enqueue_task(self.id);
        }
    }

    #[inline]
    unsafe fn into_raw_waker(data: *const TaskHandleData) -> task::RawWaker {
        task::RawWaker::new(data as *const (), &TaskHandle::WAKER_VTABLE)
    }
}

/// A [`Waker`] from a [`TaskHandle`] reference.
#[derive(Clone)]
#[repr(transparent)]
struct AsWaker<'a> {
    waker: ManuallyDrop<Waker>,
    _ref: PhantomData<&'a TaskHandle>,
}

impl Deref for AsWaker<'_> {
    type Target = Waker;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.waker
    }
}

impl TaskHandle {
    #[inline]
    fn new(executor: &'static Executor, id: usize) -> Self {
        Self(Arc::new(TaskHandleData {
            executor,
            id,
            is_queued: AtomicBool::new(false),
        }))
    }

    /// Turn this task handle into a waker.
    #[inline]
    pub fn into_waker(self) -> Waker {
        let arc_data = Arc::into_raw(self.0);
        unsafe { Waker::from_raw(TaskHandleData::into_raw_waker(arc_data)) }
    }

    /// Create a waker without increasing the reference count.
    #[inline]
    pub fn as_waker(&self) -> AsWaker<'_> {
        let arc_data = Arc::as_ptr(&self.0);
        AsWaker {
            waker: ManuallyDrop::new(unsafe {
                Waker::from_raw(TaskHandleData::into_raw_waker(arc_data))
            }),
            _ref: PhantomData,
        }
    }

    const WAKER_VTABLE: task::RawWakerVTable = task::RawWakerVTable::new(
        TaskHandle::waker_clone,
        TaskHandle::waker_wake,
        TaskHandle::waker_wake_by_ref,
        TaskHandle::waker_drop,
    );

    unsafe fn waker_clone(arc_data: *const ()) -> task::RawWaker {
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
