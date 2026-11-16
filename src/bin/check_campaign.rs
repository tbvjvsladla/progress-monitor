//! `check_campaign`: the read-only form check of a campaign. See `--help`.

fn main() {
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    std::process::exit(progress_monitor::lint::cli::main(&argv));
}
