//! `pm`: the campaign executor. See the crate documentation for the commands.

/// Stack for the command thread: JSON nested as deep as the baseline's decoder
/// took (about 10 000 levels) is read and written recursively.
const STACK_BYTES: usize = 512 << 20;

fn main() {
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| progress_monitor::jsonio::mark(&a.to_string_lossy()))
        .collect();
    let hook = argv.iter().any(|a| a == "stop-hook");
    let command = move || {
        if hook {
            // The hook must end with 0 whatever happens, even when its output cannot be written.
            let _ = std::panic::catch_unwind(|| progress_monitor::cli::main(&argv));
            0
        } else {
            progress_monitor::cli::main(&argv)
        }
    };
    let code = match run_with_large_stack(command) {
        Some(code) => code,
        None if hook => 0,
        None => 101, // a panic, as the main thread would have ended
    };
    std::process::exit(code);
}

/// Run `command` on a thread with [`STACK_BYTES`] of stack; when that thread
/// cannot be made (for example under a small address-space limit) run it here.
/// None when it panicked.
fn run_with_large_stack(command: impl FnOnce() -> i32 + Send + 'static) -> Option<i32> {
    let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(command)));
    let taken = std::sync::Arc::clone(&slot);
    let spawned = std::thread::Builder::new()
        .stack_size(STACK_BYTES)
        .spawn(move || taken.lock().ok().and_then(|mut c| c.take()).map(|c| c()));
    match spawned {
        Ok(handle) => handle.join().ok().flatten(),
        Err(_) => {
            let command = slot.lock().ok().and_then(|mut c| c.take())?;
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(command)).ok()
        }
    }
}
