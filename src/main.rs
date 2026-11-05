fn main() {
    let json = metal_slug_classics::cli::json_requested();
    if let Err(error) = metal_slug_classics::cli::run() {
        if json {
            eprintln!("{}", error.json());
        } else {
            eprintln!("error: {error}");
        }
        std::process::exit(1);
    }
}
