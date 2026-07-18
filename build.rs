use std::env;
use std::ffi::{OsStr, OsString};
use std::process::Command;

fn command_works(program: &OsStr, argument: &str) -> bool {
    Command::new(program)
        .arg(argument)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn main() {
    println!("cargo:rerun-if-env-changed=CMAKE");
    println!("cargo:rerun-if-env-changed=PROTOC");
    println!("cargo:rerun-if-changed=scripts/protoc");

    let protoc = env::var_os("PROTOC").unwrap_or_else(|| OsString::from("protoc"));
    if !command_works(&protoc, "--version") {
        panic!(
            "protobuf compiler is unavailable. The repository config should set \
             PROTOC=scripts/protoc; verify curl, unzip, and network access, or set \
             PROTOC to a working protoc binary"
        );
    }

    if env::var_os("CARGO_FEATURE_NATIVE_LLAMA").is_none() {
        return;
    }

    let cmake = env::var_os("CMAKE").unwrap_or_else(|| OsString::from("cmake"));
    if !command_works(&cmake, "--version") {
        panic!(
            "the 'native-llama' feature requires CMake to compile llama.cpp. \
             Install it with `brew install cmake`, set CMAKE to a working binary, \
             or run deterministic hash tests with `cargo test --no-default-features`"
        );
    }
}
