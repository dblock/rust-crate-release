#[path = "support/cli.rs"]
mod fixture;

#[test]
fn source_executable_uses_the_shared_release_fixtures() -> Result<(), Box<dyn std::error::Error>> {
    fixture::check(std::path::Path::new(env!("CARGO_BIN_EXE_rcr")))
}
