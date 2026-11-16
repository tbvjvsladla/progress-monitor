//! `pm`: the campaign executor. See the crate documentation for the commands.

fn main() {
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let hook = argv.iter().any(|a| a == "stop-hook");
    if hook {
        // The hook must end with 0 whatever happens, even when its output cannot be written.
        let _ = std::panic::catch_unwind(|| progress_monitor::cli::main(&argv));
        std::process::exit(0);
    }
    std::process::exit(progress_monitor::cli::main(&argv));
}
