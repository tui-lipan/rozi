fn main() {
    match hyprmux::platform::executable::run_windows_launcher() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("hyprmux launcher: {error}");
            std::process::exit(1);
        }
    }
}
