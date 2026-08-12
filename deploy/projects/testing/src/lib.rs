/// Compute the nth Fibonacci number iteratively.
///
/// Guardrails:
/// - No recursion — uses a simple loop.
/// - No Vec or arrays — only two scalar accumulator variables.
/// - No external crates.
pub fn fib(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    if n == 1 {
        return 1;
    }

    let mut prev = 0u64;
    let mut curr = 1u64;
    for _ in 2u64..=n {
        let next = prev + curr;
        prev = curr;
        curr = next;
    }
    curr
}

/// Search for `target` in the sorted slice `items` using binary search.
///
/// Returns `Some(index)` if found, `None` otherwise.
///
/// Guardrails:
/// - No recursion — uses an iterative loop.
/// - No standard library `binary_search`.
/// - Midpoint computed as `left + (right - left) / 2` to avoid integer overflow.
pub fn binary_search(items: &[i32], target: i32) -> Option<usize> {
    let mut left = 0usize;
    let mut right = items.len();

    while left < right {
        let mid = left + (right - left) / 2;
        let value = items[mid];

        if value == target {
            return Some(mid);
        } else if value < target {
            left = mid + 1;
        } else {
            right = mid;
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fib_zero() {
        assert_eq!(fib(0), 0);
    }

    #[test]
    fn test_fib_one() {
        assert_eq!(fib(1), 1);
    }

    #[test]
    fn test_fib_ten() {
        assert_eq!(fib(10), 55);
    }

    #[test]
    fn test_fib_twenty() {
        assert_eq!(fib(20), 6765);
    }

    #[test]
    fn test_bs_found_middle() {
        assert_eq!(binary_search(&[1, 3, 5, 7, 9], 5), Some(2));
    }

    #[test]
    fn test_bs_found_first() {
        assert_eq!(binary_search(&[1, 3, 5, 7, 9], 1), Some(0));
    }

    #[test]
    fn test_bs_found_last() {
        assert_eq!(binary_search(&[1, 3, 5, 7, 9], 9), Some(4));
    }

    #[test]
    fn test_bs_not_found() {
        assert_eq!(binary_search(&[1, 3, 5, 7, 9], 4), None);
    }

    #[test]
    fn test_bs_empty() {
        assert_eq!(binary_search(&[], 5), None);
    }
}
