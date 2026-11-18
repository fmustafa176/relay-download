use std::{env, path::PathBuf, process::Command};

fn main() {
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("curl_bridge.o");
    assert!(Command::new("cc")
        .args(["-O2", "-c", "src/curl_bridge.c", "-o"])
        .arg(&output)
        .status()
        .unwrap()
        .success());
    println!("cargo:rustc-link-arg={}", output.display());
    println!("cargo:rustc-link-lib=curl");
    println!("cargo:rerun-if-changed=src/curl_bridge.c");
    println!("cargo:rerun-if-env-changed=RELAY_REVISION");
}
