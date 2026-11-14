//! File-provider downloads must never hold the UI thread, including at
//! startup and while saving on close.

use std::io;
use std::marker::PhantomData;
use std::rc::Rc;

/// Restores the calling thread's policy when it goes out of scope. It
/// cannot move to another thread: the policy belongs to this one alone.
pub struct LocalIo {
    #[cfg(target_os = "macos")]
    previous: i32,
    _thread: PhantomData<Rc<()>>,
}

pub fn protect_ui_thread() -> io::Result<LocalIo> {
    #[cfg(target_os = "macos")]
    {
        let previous = unsafe { getiopolicy_np(MATERIALIZE_DATALESS_FILES, THREAD) };
        if previous < 0 || unsafe { setiopolicy_np(MATERIALIZE_DATALESS_FILES, THREAD, OFF) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(LocalIo { previous, _thread: PhantomData })
    }
    #[cfg(not(target_os = "macos"))]
    Ok(LocalIo { _thread: PhantomData })
}

/// macOS returns EDEADLK when an operation would materialize a file or
/// an intermediate directory on a thread that has opted out.
pub fn is_offloaded(error: &io::Error) -> bool {
    cfg!(target_os = "macos") && error.raw_os_error() == Some(11)
}

// sys/resource.h. A stat-before-read check alone races with eviction and
// can itself materialize intermediate directories. The kernel policy
// covers all filesystem calls, including the saves during teardown.
// https://developer.apple.com/documentation/technotes/tn3150-getting-ready-for-data-less-files
#[cfg(target_os = "macos")]
const MATERIALIZE_DATALESS_FILES: i32 = 3;
#[cfg(target_os = "macos")]
const THREAD: i32 = 1;
#[cfg(target_os = "macos")]
const OFF: i32 = 1;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn getiopolicy_np(iotype: i32, scope: i32) -> i32;
    fn setiopolicy_np(iotype: i32, scope: i32, policy: i32) -> i32;
}

#[cfg(target_os = "macos")]
impl Drop for LocalIo {
    fn drop(&mut self) {
        // The guard cannot cross threads. Restoring a policy returned by
        // the system needs no access to any file or worker.
        unsafe { setiopolicy_np(MATERIALIZE_DATALESS_FILES, THREAD, self.previous); }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn protection_is_scoped_to_the_ui_thread() {
        let policy = || unsafe { getiopolicy_np(MATERIALIZE_DATALESS_FILES, THREAD) };
        let before = policy();
        let worker_before = std::thread::spawn(policy).join().unwrap();
        {
            let _outer = protect_ui_thread().unwrap();
            assert_eq!(policy(), OFF);
            {
                let _inner = protect_ui_thread().unwrap();
                assert_eq!(policy(), OFF);
            }
            assert_eq!(policy(), OFF);
            assert_eq!(std::thread::spawn(policy).join().unwrap(), worker_before);
        }
        assert_eq!(policy(), before);
    }
}
