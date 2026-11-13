/// Computes the longest common subsequence (LCS) of sequences A and B.
/// Equality of elements is determined by the provided comparator.
///
/// The algorithm used is the O(ND) time and space algorithm from:
///
///     Myers, Eugene W. An O(ND) Difference Algorithm and Its Variations.
///     Algorithmica (1986) 1: 251. http://xmailserver.org/diff2.pdf
///
/// For understanding this algorithm,
/// <http://simplygenius.net/Article/DiffTutorial1> is also helpful.
///
/// TODO: implement the O(N) space refinement presented in the paper.
///
/// C++ writes to an output iterator; the port returns the LCS (elements of
/// `a`, cloned).
pub fn longest_common_subsequence<T: Clone>(a: &[T], b: &[T], eq: impl Fn(&T, &T) -> bool) -> Vec<T> {
    let n = a.len() as isize;
    let m = b.len() as isize;
    let d_max = n + m;

    if d_max == 0 {
        return Vec::new();
    }

    let idx = |k: isize| -> usize { (k + d_max) as usize };

    let mut vs: Vec<Vec<isize>> = Vec::new();

    // C++ uses a self-executing lambda to `return` from the inner loop.
    'outer: {
        let mut v: Vec<isize> = vec![0; (2 * d_max + 1) as usize];
        v[1] = 0;

        // Core of the algorithm: greedily find farthest-reaching D-paths for
        // increasing values of D. Store the farthest-reaching endpoints found
        // in each iteration for later reconstructing the LCS.
        let mut d = 0;
        while d <= d_max {
            let mut k = -d;
            while k <= d {
                let mut x = if k == -d || (k != d && v[idx(k - 1)] < v[idx(k + 1)]) {
                    v[idx(k + 1)] // moving down
                } else {
                    v[idx(k - 1)] + 1 // moving right
                };

                let mut y = x - k;

                while x < n && y < m && eq(&a[x as usize], &b[y as usize]) {
                    x += 1;
                    y += 1;
                }

                v[idx(k)] = x;

                if x >= n && y >= m {
                    vs.push(v);
                    break 'outer;
                }
                k += 2;
            }

            vs.push(v.clone());
            d += 1;
        }
    }

    let mut x = n;
    let mut y = m;

    let mut lcs_reverse: Vec<T> = Vec::new();

    // Reconstruct the LCS using the farthest-reaching endpoints stored above.
    let mut d = vs.len() as isize - 1;
    while x > 0 || y > 0 {
        let v = &vs[d as usize];
        let k = x - y;
        let down = k == -d || (k != d && v[idx(k - 1)] < v[idx(k + 1)]);
        let k_prev = if down { k + 1 } else { k - 1 };

        x = v[idx(k_prev)];
        y = x - k_prev;

        let mut c = v[idx(k)];
        let end = if down { x } else { x + 1 };
        while c != end {
            lcs_reverse.push(a[(c - 1) as usize].clone());
            c -= 1;
        }
        d -= 1;
    }

    lcs_reverse.reverse();
    lcs_reverse
}

#[cfg(test)]
mod tests {
    // The C++ tree has no gtest for this header; these check the algorithm.
    use super::*;

    fn lcs(a: &str, b: &str) -> String {
        let a: Vec<char> = a.chars().collect();
        let b: Vec<char> = b.chars().collect();
        longest_common_subsequence(&a, &b, |x, y| x == y).into_iter().collect()
    }

    #[test]
    fn basics() {
        assert_eq!(lcs("", ""), "");
        assert_eq!(lcs("a", ""), "");
        assert_eq!(lcs("", "a"), "");
        assert_eq!(lcs("a", "a"), "a");
        assert_eq!(lcs("abc", "abc"), "abc");
        assert_eq!(lcs("abcabba", "cbabac").len(), 4);
        assert_eq!(lcs("abc", "xyz"), "");
        assert_eq!(lcs("abcd", "acd"), "acd");
    }
}
