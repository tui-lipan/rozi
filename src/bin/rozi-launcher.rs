fn main() {
    match relswap::launcher::run("hyprmux.exe") {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("hyprmux launcher: {error}");
            std::process::exit(1);
        }
    }
}
