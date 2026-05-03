use std::io;

#[cfg(target_os = "linux")]
pub fn pin_current_thread_to_cpu(cpu_id: usize) -> io::Result<()> {
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut set);
        libc::CPU_SET(cpu_id, &mut set);
        let ret = libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
        if ret != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn pin_current_thread_to_cpu(cpu_id: usize) -> io::Result<()> {
    use std::os::raw::c_int;

    const THREAD_AFFINITY_POLICY: c_int = 4;

    #[repr(C)]
    struct ThreadAffinityPolicy {
        affinity_tag: c_int,
    }

    extern "C" {
        fn thread_policy_set(
            thread: libc::thread_t,
            flavor: c_int,
            policy_info: *const ThreadAffinityPolicy,
            count: c_int,
        ) -> c_int;
        fn mach_thread_self() -> libc::thread_t;
    }

    let policy = ThreadAffinityPolicy {
        affinity_tag: cpu_id as c_int,
    };
    let ret = unsafe {
        thread_policy_set(
            mach_thread_self(),
            THREAD_AFFINITY_POLICY,
            &policy,
            1,
        )
    };
    if ret != 0 {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("thread_policy_set failed: {ret}"),
        ))
    } else {
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn pin_current_thread_to_cpu(_cpu_id: usize) -> io::Result<()> {
    Ok(())
}
