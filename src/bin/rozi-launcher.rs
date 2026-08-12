fn main() {
    match relswap::launcher::run("rozi.exe") {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("rozi launcher: {error}");
            std::process::exit(1);
        }
    }
}
