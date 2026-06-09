//! Applies the linker script to the bare-metal kernel binary.
//!
//! Gated on the riscv target: host builds (`cargo build`, `cargo test`)
//! must not receive `-T`, or the host linker would fail.

fn main() {
    println!("cargo:rerun-if-changed=kernel.ld");
    let target = std::env::var("TARGET").unwrap_or_default();
    if target == "riscv64gc-unknown-none-elf" {
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        println!("cargo:rustc-link-arg-bins=-T{dir}/kernel.ld");
    }
}
