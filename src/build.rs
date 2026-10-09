use std::env;
use std::fs::File;
use std::io::Write;
use std::path::Path;

fn main() {
    // Get Cargo package version
    let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".into());
    // Get current UTC timestamp as RFC3339
    let now = chrono::Utc::now().to_rfc3339();
    // Write a file that will be compiled into the binary
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let dest_path = Path::new(&out_dir).join("build_info.rs");
    let mut f = File::create(&dest_path).expect("could not create build_info.rs");
    writeln!(f, "pub const BUILD_VERSION: &str = \"{}\";", version).unwrap();
    writeln!(f, "pub const BUILD_TIME: &str = \"{}\";", now).unwrap();
}
