//! Sorting and searching records in place, in the caller's memory: a heap sort (no
//! recursion, no scratch) and the partition point of a sorted run. What the workbook reads
//! by key — a relationship by its id, a number format by its id — is bounded by nothing but
//! the file, so a lookup that walked the records would make a hostile file quadratic.

/// Sorts `count` records of `width` elements each, held back to back in `items`, by `less`
/// over record indices. `swap` exchanges two records; both closures see the whole table.
pub fn heap_sort<T>(
    items: &mut [T],
    count: usize,
    less: impl Fn(&[T], usize, usize) -> bool,
    swap: impl Fn(&mut [T], usize, usize),
) {
    let sift = |items: &mut [T], mut root: usize, end: usize| {
        loop {
            let mut child = match root.checked_mul(2).and_then(|c| c.checked_add(1)) {
                Some(child) if child < end => child,
                _ => return,
            };
            if child + 1 < end && less(items, child, child + 1) {
                child += 1;
            }
            if !less(items, root, child) {
                return;
            }
            swap(items, root, child);
            root = child;
        }
    };
    let mut start = count / 2;
    while start > 0 {
        start -= 1;
        sift(items, start, count);
    }
    let mut end = count;
    while end > 1 {
        end -= 1;
        swap(items, 0, end);
        sift(items, 0, end);
    }
}

/// The first index in `0..count` for which `before(index)` is false, given that it is true
/// for every index below some point and false from there on.
pub fn partition_point(count: usize, before: impl Fn(usize) -> bool) -> usize {
    let (mut low, mut high) = (0usize, count);
    while low < high {
        let mid = low + (high - low) / 2;
        if before(mid) {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    low
}
