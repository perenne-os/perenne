//! Memory management: physical frame allocation and Sv39 paging.
//!
//! Same layout discipline as [`crate::trap`]: pure logic is ungated and
//! unit-tested on the host; everything touching real memory or CSRs is
//! gated to `target_arch = "riscv64"`.

pub mod frame;
