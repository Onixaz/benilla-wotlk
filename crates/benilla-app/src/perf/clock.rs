//! The CPU clocks the instruments are denominated in: process, main thread, and machine.

/// Whole-process CPU seconds so far, user + system across every thread: work per frame, which
/// other load on the machine barely moves, unlike wall frame time. `None` off Unix and Windows.
pub(crate) fn process_cpu_secs() -> Option<f64> {
    #[cfg(unix)]
    {
        // SAFETY: `getrusage` fully writes the out-param and reads nothing from it; zeroed is
        // valid for a struct of plain integers.
        unsafe {
            let mut ru: libc::rusage = std::mem::zeroed();
            if libc::getrusage(libc::RUSAGE_SELF, &mut ru) != 0 {
                return None;
            }
            let secs = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 * 1e-6;
            Some(secs(ru.ru_utime) + secs(ru.ru_stime))
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        use windows_sys::Win32::System::WindowsProgramming::QueryProcessCycleTime;
        if let Some(hz) = cycle_hz() {
            let mut cycles = 0u64;
            // SAFETY: writes one `u64`; the pseudo-handle is never closed.
            if unsafe { QueryProcessCycleTime(GetCurrentProcess(), &mut cycles) } != 0 {
                return Some(cycles as f64 / hz);
            }
        }
        // SAFETY: `GetProcessTimes` fully writes four plain-integer `FILETIME`s and reads none;
        // the `GetCurrentProcess` pseudo-handle is never closed.
        unsafe {
            let mut creation = std::mem::zeroed();
            let mut exit = std::mem::zeroed();
            let mut kernel = std::mem::zeroed();
            let mut user = std::mem::zeroed();
            if GetProcessTimes(
                GetCurrentProcess(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            ) == 0
            {
                return None;
            }
            Some(filetime_secs(kernel) + filetime_secs(user))
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

/// The rate `Query{Thread,Process}CycleTime` count at: they read the invariant TSC at each
/// context switch, so they charge exactly the cycles run, where `Get{Thread,Process}Times` charge a
/// whole 15.6 ms tick to whichever thread the timer interrupt finds (a frame-paced thread that
/// blocks before each tick reads near zero). Calibrated once against the monotonic clock; `None`
/// off x86-64, where the times APIs stand in.
#[cfg(windows)]
fn cycle_hz() -> Option<f64> {
    static HZ: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();
    *HZ.get_or_init(|| {
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: `_rdtsc` reads the time-stamp counter, present on every x86-64 CPU.
            let tsc = || unsafe { core::arch::x86_64::_rdtsc() };
            let (t0, c0) = (std::time::Instant::now(), tsc());
            std::thread::sleep(std::time::Duration::from_millis(10));
            let (dt, dc) = (t0.elapsed().as_secs_f64(), tsc().wrapping_sub(c0));
            (dt > 0.0 && dc > 0).then(|| dc as f64 / dt)
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            None
        }
    })
}

/// A `FILETIME` as an integer of 100-nanosecond ticks.
#[cfg(windows)]
fn filetime_ticks(t: windows_sys::Win32::Foundation::FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

/// A `FILETIME` as seconds.
#[cfg(windows)]
fn filetime_secs(t: windows_sys::Win32::Foundation::FILETIME) -> f64 {
    filetime_ticks(t) as f64 * 1e-7
}

/// The process's `(minor, major)` page faults so far. Minor faults are kernel time no user-mode
/// sampler sees; per frame they say whether [`thread_cpu_table`]'s `sys` is allocator churn.
pub(crate) fn process_faults() -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        // SAFETY: as in `process_cpu_secs`: `getrusage` fills the out-param completely.
        unsafe {
            let mut ru: libc::rusage = std::mem::zeroed();
            if libc::getrusage(libc::RUSAGE_SELF, &mut ru) != 0 {
                return None;
            }
            Some((ru.ru_minflt as u64, ru.ru_majflt as u64))
        }
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// CPU seconds consumed by the calling thread. A caller that wants the main thread's must pin
/// itself there with a [`NonSendMarker`](bevy::ecs::system::NonSendMarker) param, or it reads
/// whichever worker ran the system. `None` off Unix and Windows.
pub(crate) fn main_thread_cpu_secs() -> Option<f64> {
    #[cfg(unix)]
    {
        // SAFETY: `clock_gettime` fully writes the out-param and reads nothing from it; zeroed is
        // valid for two plain integers.
        unsafe {
            let mut ts: libc::timespec = std::mem::zeroed();
            if libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) != 0 {
                return None;
            }
            Some(ts.tv_sec as f64 + ts.tv_nsec as f64 * 1e-9)
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
        use windows_sys::Win32::System::WindowsProgramming::QueryThreadCycleTime;
        if let Some(hz) = cycle_hz() {
            let mut cycles = 0u64;
            // SAFETY: writes one `u64`; the pseudo-handle is valid on the calling thread.
            if unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut cycles) } != 0 {
                return Some(cycles as f64 / hz);
            }
        }
        // SAFETY: as in `process_cpu_secs`; `GetCurrentThread` is a pseudo-handle valid on the
        // thread that asked for it.
        unsafe {
            let mut creation = std::mem::zeroed();
            let mut exit = std::mem::zeroed();
            let mut kernel = std::mem::zeroed();
            let mut user = std::mem::zeroed();
            if GetThreadTimes(
                GetCurrentThread(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            ) == 0
            {
                return None;
            }
            Some(filetime_secs(kernel) + filetime_secs(user))
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

/// Cumulative machine-wide CPU ticks as `(busy, total)`, every core and process; diff two samples
/// for the busy fraction. Machine load inflates `cpu_ms` too (cache and memory-bandwidth
/// contention), so a leg stamps this and only legs at similar load compare. Load average is no
/// substitute: a 1-minute decaying mean lags a build burst. `None` off macOS and Windows.
///
/// The `deprecated` allow is `libc::mach_host_self`: `mach2` has the port but not
/// `host_statistics64` or its types, so `libc` stays the one source for the call.
#[allow(deprecated)]
pub(crate) fn system_cpu_ticks() -> Option<(u64, u64)> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: `host_statistics64` fills at most `count` u32s and reads nothing; the struct is
        // plain integers, so zeroed is valid. `mach_host_self` is the global host port, with no
        // send right to deallocate.
        unsafe {
            let mut info: libc::host_cpu_load_info = std::mem::zeroed();
            let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
            if libc::host_statistics64(
                libc::mach_host_self(),
                libc::HOST_CPU_LOAD_INFO,
                (&raw mut info).cast(),
                &mut count,
            ) != libc::KERN_SUCCESS
            {
                return None;
            }
            let total: u64 = info.cpu_ticks.iter().map(|&x| u64::from(x)).sum();
            let idle = u64::from(info.cpu_ticks[libc::CPU_STATE_IDLE as usize]);
            Some((total - idle, total))
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::GetSystemTimes;
        // SAFETY: `GetSystemTimes` writes three fully-initialised `FILETIME`s and reads nothing.
        unsafe {
            let mut idle = std::mem::zeroed();
            let mut kernel = std::mem::zeroed();
            let mut user = std::mem::zeroed();
            if GetSystemTimes(&mut idle, &mut kernel, &mut user) == 0 {
                return None;
            }
            // All three are summed across every processor, and kernel time includes idle.
            let total = filetime_ticks(kernel) + filetime_ticks(user);
            Some((total.saturating_sub(filetime_ticks(idle)), total))
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        None
    }
}

/// CPU seconds so far per thread of this process, as `(pthread id, name, user, system)`, keyed so
/// two snapshots subtract; an unnamed thread reads `"?"`. macOS only; one kernel round-trip per
/// thread, so taken per probe window, never per frame.
pub(crate) fn thread_cpu_table() -> Option<Vec<(usize, String, f64, f64)>> {
    // Declared here rather than taken from `libc`'s deprecated mach bindings; both are
    // libSystem's.
    #[cfg(target_os = "macos")]
    extern "C" {
        static mach_task_self_: libc::mach_port_t;
        fn mach_port_deallocate(
            task: libc::mach_port_t,
            name: libc::mach_port_t,
        ) -> libc::kern_return_t;
    }
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CStr;
        let mut out = Vec::new();
        // SAFETY: `task_threads` returns a kernel-allocated array of thread ports and its count;
        // each port is read into a count-checked `thread_basic_info`, and every port and the
        // array are released after.
        unsafe {
            let task = mach_task_self_;
            let mut list: libc::thread_act_array_t = std::ptr::null_mut();
            let mut count: libc::mach_msg_type_number_t = 0;
            if libc::task_threads(task, &mut list, &mut count) != libc::KERN_SUCCESS {
                return None;
            }
            for i in 0..count as usize {
                let port = *list.add(i);
                let mut info: libc::thread_basic_info = std::mem::zeroed();
                let mut n = libc::THREAD_BASIC_INFO_COUNT;
                let kr = libc::thread_info(
                    port,
                    libc::THREAD_BASIC_INFO as libc::thread_flavor_t,
                    (&mut info as *mut libc::thread_basic_info).cast(),
                    &mut n,
                );
                if kr == libc::KERN_SUCCESS {
                    let secs = |t: libc::time_value_t| {
                        f64::from(t.seconds) + f64::from(t.microseconds) * 1e-6
                    };
                    let (user, system) = (secs(info.user_time), secs(info.system_time));
                    let pthread = libc::pthread_from_mach_thread_np(port);
                    let mut name = [0 as libc::c_char; 64];
                    let label = if pthread != 0
                        && libc::pthread_getname_np(pthread, name.as_mut_ptr(), name.len()) == 0
                    {
                        let s = CStr::from_ptr(name.as_ptr()).to_string_lossy();
                        if s.is_empty() {
                            "?".to_string()
                        } else {
                            s.into_owned()
                        }
                    } else {
                        "?".to_string()
                    };
                    out.push((pthread as usize, label, user, system));
                }
                mach_port_deallocate(task, port);
            }
            libc::vm_deallocate(
                task,
                list as libc::vm_address_t,
                (count as usize * std::mem::size_of::<libc::thread_act_t>()) as libc::vm_size_t,
            );
        }
        Some(out)
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// The thread clock moves with work, never past the wall, and not while asleep.
    #[test]
    fn the_thread_clock_counts_run_time_only() {
        let Some(c0) = main_thread_cpu_secs() else {
            return; // no thread clock on this platform
        };
        let t0 = Instant::now();
        let mut c1 = c0;
        while c1 - c0 < 0.02 && t0.elapsed() < Duration::from_secs(2) {
            std::hint::black_box((0..1000u64).sum::<u64>());
            c1 = main_thread_cpu_secs().unwrap();
        }
        let wall = t0.elapsed().as_secs_f64();
        assert!(c1 - c0 >= 0.02, "20 ms of spinning never registered");
        assert!(
            c1 - c0 <= wall + 0.001,
            "{:.1} ms of CPU in {:.1} ms of wall: a tick-sampled clock",
            (c1 - c0) * 1e3,
            wall * 1e3
        );
        std::thread::sleep(Duration::from_millis(30));
        let slept = main_thread_cpu_secs().unwrap() - c1;
        assert!(slept < 0.003, "{:.1} ms charged to a sleep", slept * 1e3);
    }
}
