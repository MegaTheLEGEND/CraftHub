use std::{env, fs::File, io::Write, path::Path};

fn main() {
    let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".into());
    let now = chrono::Utc::now().to_rfc3339();
    let out = Path::new(&env::var("OUT_DIR").expect("OUT_DIR")).join("build_info.rs");
    let mut f = File::create(out).expect("create build_info.rs");
    writeln!(f, "pub const BUILD_VERSION: &str = \"{version}\";").unwrap();
    writeln!(f, "pub const BUILD_TIME: &str = \"{now}\";").unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
