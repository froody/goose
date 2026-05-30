fn main() {
    #[cfg(unix)]
    #[allow(unsafe_code)]
    // nosemgrep: unsafe-block
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let code = match rtk::run_cli() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("rtk: {:#}", e);
            1
        }
    };
    std::process::exit(code);
}
