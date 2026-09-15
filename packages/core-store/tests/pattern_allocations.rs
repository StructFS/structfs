use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use structfs_core_store::{matches_prefix_suffix, path, PathPattern};
struct Counting;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;
#[test]
fn suffix_matching_allocates_nothing() {
    let prefix = path!("accounts");
    let suffix = path!("provider");
    let empty = path!("");
    let paths = [
        path!("accounts/a/provider"),
        path!("accounts/provider"),
        path!("other/a/provider"),
        path!("accounts/a/provider_other"),
        path!(""),
    ];
    let pattern = PathPattern::prefix_suffix_with_min_middle(prefix.clone(), suffix.clone(), 1);
    let before = ALLOCS.load(Ordering::SeqCst);
    for _ in 0..1000 {
        for p in &paths {
            std::hint::black_box(pattern.matches(std::hint::black_box(p)));
            std::hint::black_box(matches_prefix_suffix(p, &prefix, &empty, 0));
            std::hint::black_box(matches_prefix_suffix(p, &empty, &suffix, usize::MAX));
        }
    }
    assert_eq!(ALLOCS.load(Ordering::SeqCst), before);
    assert!(pattern.matches(&paths[0]));
    for p in &paths[1..] {
        assert!(!pattern.matches(p));
    }
}
