//! What a serving process holds, kept small. A process that serves pages and now and then draws
//! one large picture should hold what its pages need and give the rest back.
//!
//! On Linux with glibc, two things decide that beyond what the program allocates: glibc gives
//! every thread an arena of its own, so eight threads that each once drew a large page hold eight
//! arenas; and memory freed inside an arena stays with the process until it is trimmed. Both are
//! set here, through glibc's own calls, so nothing needs to be set outside the program.

#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod glibc {
    extern "C" {
        fn mallopt(param: i32, value: i32) -> i32;
        fn malloc_trim(pad: usize) -> i32;
    }
    /// `M_ARENA_MAX` in malloc.h.
    const M_ARENA_MAX: i32 = -8;

    pub fn arenas(n: i32) {
        // Safety: mallopt takes two integers and changes how later allocations are placed.
        unsafe {
            mallopt(M_ARENA_MAX, n);
        }
    }

    pub fn trim() {
        // Safety: malloc_trim takes the padding to keep and returns what is free to the system.
        unsafe {
            malloc_trim(0);
        }
    }
}

/// Before the serving threads start: two arenas for all of them.
pub fn serving() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    glibc::arenas(2);
}

/// What was freed given back to the system: after a picture drawn ahead, or trackers closed.
pub fn give_back() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    glibc::trim();
}

/// The thread that closes trackers nobody asks for, and gives back what they held.
pub fn spawn_janitor(close: fn() -> usize) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
        if close() > 0 {
            give_back();
        }
    });
}
