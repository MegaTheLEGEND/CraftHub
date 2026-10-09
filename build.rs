use std::env;
use std::fs::File;
use std::io::Write;
use std::path::Path;

fn main() {
    let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".into());
    let now = chrono::Utc::now().to_rfc3339();
    let out_path = Path::new("src").join("build_info.rs");
    let mut f = File::create(&out_path).expect("could not create src/build_info.rs");
    writeln!(f, "pub const BUILD_VERSION: &str = \"{}\";", version).unwrap();
    writeln!(f, "pub const BUILD_TIME: &str = \"{}\";", now).unwrap();
}
