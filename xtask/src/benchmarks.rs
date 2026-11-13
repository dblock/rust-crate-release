use crate::{Result, run};
use serde_json::json;
use std::{fs, path::Path, time::Instant};

pub fn test_suites(root: &Path) -> Result {
    run(
        root,
        "cargo",
        &["test", "--locked", "--workspace", "--no-run"],
    )?;
    let mut measurements = Vec::new();
    for (name, arguments) in [
        ("library", vec!["-p", "rust-crate-release", "--lib"]),
        ("cli", vec!["-p", "rust-crate-release", "--test", "cli"]),
        (
            "release-runner",
            vec!["-p", "xtask", "--bin", "rcr-release"],
        ),
        ("maintenance", vec!["-p", "xtask", "--bin", "xtask"]),
        ("doctests", vec!["--workspace", "--doc"]),
    ] {
        let mut args = vec!["test", "--locked"];
        args.extend(arguments);
        let start = Instant::now();
        run(root, "cargo", &args)?;
        measurements.push(json!({
            "suite": name,
            "elapsed_seconds": start.elapsed().as_secs_f64(),
        }));
    }
    let report = json!({
        "profile": "debug",
        "notes": "One sequential run per suite after cargo test --no-run; includes Cargo startup, and rustdoc compilation for doctests.",
        "measurements": measurements,
    });
    let directory = root.join("target/benchmarks");
    fs::create_dir_all(&directory)?;
    let path = directory.join("test-suites.json");
    fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
    println!("\nTest Suite Wall Times");
    for measurement in &measurements {
        println!(
            "{:<16} {:>9.3} s",
            measurement["suite"].as_str().unwrap(),
            measurement["elapsed_seconds"].as_f64().unwrap()
        );
    }
    println!("Saved {}", path.display());
    Ok(())
}
