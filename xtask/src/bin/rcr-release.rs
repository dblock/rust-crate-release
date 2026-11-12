#[path = "../release.rs"]
mod release;

fn main() -> std::process::ExitCode {
    rust_crate_release::cli::main_with(release::configure)
}
