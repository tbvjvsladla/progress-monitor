//! `check_campaign`: the read-only form check of a campaign. See `--help`.

fn main() {
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| progress_monitor::lint::py_text(&a))
        .collect();
    std::process::exit(progress_monitor::lint::cli::main(&argv));
}
