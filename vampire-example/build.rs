use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let vampire_dir = Path::new(&out_dir).join("vampire_tests");

    // Create the directory for fragment files
    fs::create_dir_all(&vampire_dir).unwrap();

    println!("cargo:rerun-if-changed=src/lib.rs");
}
