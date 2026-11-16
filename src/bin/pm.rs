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
    let run = std::thread::Builder::new()
        .stack_size(STACK_BYTES)
        .spawn(move || {
            if hook {
                // The hook must end with 0 whatever happens, even when its output cannot be written.
                let _ = std::panic::catch_unwind(|| progress_monitor::cli::main(&argv));
                0
            } else {
                progress_monitor::cli::main(&argv)
            }
        });
    let code = match run.map(std::thread::JoinHandle::join) {
        Ok(Ok(code)) => code,
        _ if hook => 0,
        _ => 101, // a panic, as the main thread would have ended
    };
    std::process::exit(code);
}
