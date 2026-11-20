//! The global allocator: the system's, except that large blocks come
//! straight from the kernel and go straight back when freed. The system
//! allocator keeps freed large blocks for reuse, so the pixels of each
//! wallpaper the daemon shows (tens of MB) would stay in its memory.

use std::alloc::{GlobalAlloc, Layout, System};

/// From this size on, blocks are mapped from the kernel.
const LARGE: usize = 1 << 20;

pub struct Allocator;

impl Allocator {
    fn is_large(layout: Layout) -> bool {
        // Mappings are page aligned, which covers any alignment Rust asks
        // for in practice; larger ones stay with the system allocator.
        layout.size() >= LARGE && layout.align() <= 4096
    }
}

// SAFETY: large blocks are mapped and unmapped with the same size, which
// `dealloc` gets from the same layout `alloc` got; everything else is the
// system allocator's.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !Self::is_large(layout) {
            // SAFETY: forwarded with the caller's guarantees.
            return unsafe { System.alloc(layout) };
        }
        // SAFETY: an anonymous private mapping of the requested size.
        let pointer = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                layout.size(),
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        if pointer == libc::MAP_FAILED {
            std::ptr::null_mut()
        } else {
            pointer.cast()
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if Self::is_large(layout) {
            // SAFETY: fresh anonymous mappings are zeroed by the kernel.
            unsafe { self.alloc(layout) }
        } else {
            // SAFETY: forwarded with the caller's guarantees.
            unsafe { System.alloc_zeroed(layout) }
        }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if Self::is_large(layout) {
            // SAFETY: `pointer` is a mapping of this size made by `alloc`.
            unsafe { libc::munmap(pointer.cast(), layout.size()) };
        } else {
            // SAFETY: forwarded with the caller's guarantees.
            unsafe { System.dealloc(pointer, layout) };
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn large_blocks_work_like_any_other() {
        // Goes through the global allocator, which is this one.
        let mut big = vec![1u8; 20 << 20];
        big.extend_from_slice(&[2; 4096]);
        assert_eq!((big[0], big[(20 << 20) + 10]), (1, 2));
        big.truncate(100);
        big.shrink_to_fit();
        assert_eq!(big, vec![1; 100]);
        let zeroed = vec![0u8; 16 << 20];
        assert!(zeroed.iter().all(|&b| b == 0));
    }
}
