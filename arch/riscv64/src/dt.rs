//! Device tree (flattened, FDT) parsing.
//!
//! OpenSBI passes the address of the firmware's flattened device tree in
//! `a1` (the `dtb` argument to `kmain`). We read just what the kernel needs
//! to stop hardcoding QEMU's machine: RAM base/size (the `/memory` node's
//! `reg`) and the timer frequency (`timebase-frequency`). Pure parsing is
//! host-tested; `from_ptr` (gated) wraps a raw firmware pointer.

/// What the kernel learns from the device tree at boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MachineInfo {
    pub ram_base: usize,
    pub ram_size: usize,
    pub timebase_hz: u64,
    pub uart_base: usize,
    pub uart_reg_shift: u32,
    pub rtc_base: usize,
    /// Bases of the `virtio,mmio` transport slots (QEMU `virt` exposes 8).
    pub virtio_mmio: [usize; 8],
    pub virtio_mmio_count: usize,
}

const FDT_MAGIC: u32 = 0xd00d_feed;
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;

/// Big-endian u32 at byte offset `off`, bounds-checked.
fn be_u32(buf: &[u8], off: usize) -> Option<u32> {
    let b = buf.get(off..off + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// Length (excluding NUL) of the null-terminated string at `off`.
fn cstr_len(buf: &[u8], off: usize) -> Option<usize> {
    let mut i = off;
    while *buf.get(i)? != 0 {
        i += 1;
    }
    Some(i - off)
}

/// Read `cells` big-endian u32s starting at `off` in `val` as one integer.
fn read_cells(val: &[u8], off: usize, cells: usize) -> Option<u64> {
    let mut n: u64 = 0;
    for i in 0..cells {
        n = (n << 32) | be_u32(val, off + i * 4)? as u64;
    }
    Some(n)
}

/// Parse an FDT blob for [`MachineInfo`]: RAM (`/memory` `reg`), the timer
/// frequency (`timebase-frequency`), the console UART (the `ns16550`-
/// compatible node's `reg` base + `reg-shift`), and the RTC (the `goldfish`-
/// compatible node's `reg` base). Returns `None` on a bad magic, a
/// truncated/oversized field (every offset is bounds-checked, so a malformed
/// blob never reads out of bounds), or any missing value. The `/memory`
/// `reg` is decoded using the root's `#address-cells`/`#size-cells`
/// (default 2/2 per the FDT spec). Memory is matched by node name; the UART
/// and RTC by their `compatible` property (committed at `END_NODE`).
pub fn parse(dtb: &[u8]) -> Option<MachineInfo> {
    if be_u32(dtb, 0)? != FDT_MAGIC {
        return None;
    }
    let off_struct = be_u32(dtb, 8)? as usize;
    let off_strings = be_u32(dtb, 12)? as usize;

    let mut pos = off_struct;
    let mut depth: usize = 0;
    let mut is_mem = [false; 32]; // is_mem[d] = node at depth d is "memory*"
    let mut addr_cells: u32 = 2;
    let mut size_cells: u32 = 2;
    let mut ram: Option<(usize, usize)> = None;
    let mut timebase: Option<u64> = None;

    // The UART is matched by its `compatible` property (which can appear in
    // any order within a node), so buffer per-node state and commit it when
    // the node closes — unlike the name-matched `/memory` node.
    let mut node_is_uart = false;
    let mut node_reg: Option<usize> = None;
    let mut node_shift: u32 = 0;
    let mut uart: Option<(usize, u32)> = None;
    let mut node_is_rtc = false;
    let mut rtc: Option<usize> = None;
    let mut node_is_virtio = false;
    let mut virtio_mmio = [0usize; 8];
    let mut virtio_count = 0usize;

    loop {
        let tok = be_u32(dtb, pos)?;
        pos += 4;
        match tok {
            FDT_BEGIN_NODE => {
                let name_len = cstr_len(dtb, pos)?;
                let name = dtb.get(pos..pos + name_len)?;
                depth += 1;
                if depth < is_mem.len() {
                    is_mem[depth] = name.starts_with(b"memory");
                }
                node_is_uart = false;
                node_reg = None;
                node_shift = 0;
                node_is_rtc = false;
                node_is_virtio = false;
                pos = (pos + name_len + 1 + 3) & !3; // past name + NUL, 4-pad
            }
            FDT_END_NODE => {
                if node_is_uart && uart.is_none() {
                    if let Some(b) = node_reg {
                        uart = Some((b, node_shift));
                    }
                }
                if node_is_rtc && rtc.is_none() {
                    if let Some(b) = node_reg {
                        rtc = Some(b);
                    }
                }
                if node_is_virtio {
                    if let Some(b) = node_reg {
                        if virtio_count < virtio_mmio.len() {
                            virtio_mmio[virtio_count] = b;
                            virtio_count += 1;
                        }
                    }
                }
                depth = depth.checked_sub(1)?;
            }
            FDT_PROP => {
                let len = be_u32(dtb, pos)? as usize;
                let nameoff = be_u32(dtb, pos + 4)? as usize;
                let val_off = pos + 8;
                let val = dtb.get(val_off..val_off + len)?;
                let pname_len = cstr_len(dtb, off_strings + nameoff)?;
                let pname = dtb.get(off_strings + nameoff..off_strings + nameoff + pname_len)?;

                if depth == 1 && len >= 4 {
                    if pname == b"#address-cells" {
                        addr_cells = be_u32(val, 0)?;
                    } else if pname == b"#size-cells" {
                        size_cells = be_u32(val, 0)?;
                    }
                }
                if pname == b"timebase-frequency" && len >= 4 {
                    timebase = Some(be_u32(val, 0)? as u64);
                }
                if pname == b"reg" && len >= addr_cells as usize * 4 {
                    let base = read_cells(val, 0, addr_cells as usize)? as usize;
                    if depth < is_mem.len()
                        && is_mem[depth]
                        && len >= (addr_cells + size_cells) as usize * 4
                    {
                        let sz = read_cells(val, addr_cells as usize * 4, size_cells as usize)? as usize;
                        ram = Some((base, sz));
                    }
                    node_reg = Some(base);
                }
                if pname == b"compatible" && val.windows(7).any(|w| w == b"ns16550") {
                    node_is_uart = true;
                }
                if pname == b"compatible" && val.windows(8).any(|w| w == b"goldfish") {
                    node_is_rtc = true;
                }
                if pname == b"compatible" && val.windows(11).any(|w| w == b"virtio,mmio") {
                    node_is_virtio = true;
                }
                if pname == b"reg-shift" && len >= 4 {
                    node_shift = be_u32(val, 0)?;
                }
                pos = (val_off + len + 3) & !3; // past value, 4-pad
            }
            FDT_NOP => {}
            FDT_END => break,
            _ => return None,
        }
    }

    let (uart_base, uart_reg_shift) = uart?;
    Some(MachineInfo {
        ram_base: ram?.0,
        ram_size: ram?.1,
        timebase_hz: timebase?,
        uart_base,
        uart_reg_shift,
        rtc_base: rtc?,
        virtio_mmio,
        virtio_mmio_count: virtio_count,
    })
}

/// Parse the device tree at physical pointer `ptr` (the firmware `dtb`
/// argument). Reads the header's `totalsize` to bound the blob. Panics if
/// the device tree is invalid or missing the values we need — QEMU always
/// supplies a valid one, so this is a loud safety net.
///
/// # Safety
/// `ptr` must address a valid FDT blob; called once in early boot with the
/// MMU off, before the frame allocator touches its memory.
#[cfg(target_arch = "riscv64")]
pub unsafe fn from_ptr(ptr: usize) -> MachineInfo {
    // Header prefix: magic @0, totalsize @4 (both big-endian u32).
    let header = unsafe { core::slice::from_raw_parts(ptr as *const u8, 8) };
    assert_eq!(be_u32(header, 0), Some(FDT_MAGIC), "dtb: bad magic");
    let totalsize = be_u32(header, 4).expect("dtb: short header") as usize;
    let blob = unsafe { core::slice::from_raw_parts(ptr as *const u8, totalsize) };
    parse(blob).expect("device tree invalid or missing memory/timebase")
}

#[cfg(test)]
mod tests {
    use super::*;
    const DTB: &[u8] = include_bytes!("../tests/fixtures/qemu-virt.dtb");

    #[test]
    fn parses_qemu_virt() {
        let mi = parse(DTB).expect("should parse");
        assert_eq!(mi.ram_base, 0x8000_0000, "ram base");
        assert_eq!(mi.ram_size, 128 * 1024 * 1024, "ram size 128 MiB");
        assert_eq!(mi.timebase_hz, 10_000_000, "timebase 10 MHz");
        assert_eq!(mi.uart_base, 0x1000_0000, "uart base");
        assert_eq!(mi.uart_reg_shift, 0, "uart reg-shift");
        assert_eq!(mi.rtc_base, 0x10_1000, "rtc base");
        assert_eq!(mi.virtio_mmio_count, 8, "QEMU virt has 8 virtio-mmio slots");
        assert!(mi.virtio_mmio[..mi.virtio_mmio_count].contains(&0x1000_1000), "lowest slot");
        assert!(mi.virtio_mmio[..mi.virtio_mmio_count].contains(&0x1000_8000), "highest slot (where -device attaches)");
    }

    #[test]
    fn rejects_bad_magic() {
        assert_eq!(parse(&[0u8; 16]), None);
    }

    #[test]
    fn rejects_truncated_blob() {
        // A valid header but the struct block cut short -> None, not a panic.
        assert_eq!(parse(&DTB[..64]), None);
    }
}
