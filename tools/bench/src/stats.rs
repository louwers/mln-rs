//! The statistics of a comparison.

/// The median of `values`, which must not be empty.
pub(crate) fn median(values: &[f64]) -> f64 {
    assert!(!values.is_empty(), "the median of nothing");
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) { f64::midpoint(sorted[middle - 1], sorted[middle]) } else { sorted[middle] }
}

/// The largest value of `values` less the smallest, or 0 for none.
pub(crate) fn range(values: &[f64]) -> f64 {
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    if values.is_empty() { 0.0 } else { max - min }
}

/// The two-sided p-value of the Mann-Whitney U test that `a` and `b` come
/// from the same distribution. Exact, by counting the orderings of the
/// samples, while both have at most 20 values; through the normal
/// approximation with a continuity correction beyond. Ties count half.
pub(crate) fn mann_whitney(a: &[f64], b: &[f64]) -> f64 {
    let (n, m) = (a.len(), b.len());
    assert!(n > 0 && m > 0, "Mann-Whitney U of an empty sample");
    // U counts the pairs in which `a`'s value is the larger, twice so that
    // a tie counts one.
    let twice_u: usize =
        a.iter().map(|x| b.iter().map(|y| if x > y { 2 } else { usize::from(x == y) }).sum::<usize>()).sum();
    let u = twice_u as f64 / 2.0;
    let mean = (n * m) as f64 / 2.0;
    if n <= 20 && m <= 20 {
        // How many orderings of the n + m values give each U.
        let counts = u_distribution(n, m);
        let total: f64 = counts.iter().sum();
        let distance = (u - mean).abs();
        let extreme: f64 = counts
            .iter()
            .enumerate()
            .filter(|&(value, _)| (value as f64 - mean).abs() >= distance - 1e-9)
            .map(|(_, count)| count)
            .sum();
        return (extreme / total).min(1.0);
    }
    let sd = ((n * m * (n + m + 1)) as f64 / 12.0).sqrt();
    let z = ((u - mean).abs() - 0.5).max(0.0) / sd;
    (2.0 * normal_tail(z)).min(1.0)
}

/// `counts[u]`: how many of the orderings of `n` values of one sample and
/// `m` of another give U = u.
fn u_distribution(n: usize, m: usize) -> Vec<f64> {
    // f[i][j][u], built up over i and j: the last value is either the first
    // sample's, which then beats all j of the second, or the second's.
    let max = n * m;
    let mut previous: Vec<Vec<f64>> = vec![vec![0.0; max + 1]; m + 1];
    for row in &mut previous {
        row[0] = 1.0;
    }
    for _ in 0..n {
        let mut current: Vec<Vec<f64>> = vec![vec![0.0; max + 1]; m + 1];
        current[0][0] = 1.0;
        for j in 1..=m {
            for u in 0..=max {
                let first = if u >= j { previous[j][u - j] } else { 0.0 };
                current[j][u] = first + current[j - 1][u];
            }
        }
        previous = current;
    }
    previous.swap_remove(m)
}

/// P(Z > z) of the standard normal distribution.
fn normal_tail(z: f64) -> f64 {
    0.5 * erfc(z / std::f64::consts::SQRT_2)
}

/// The complementary error function, to about 1e-7 (Numerical Recipes'
/// `erfcc`).
fn erfc(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.5 * x.abs());
    let y = t
        * (-x * x - 1.265_512_23
            + t * (1.000_023_68
                + t * (0.374_091_96
                    + t * (0.096_784_18
                        + t * (-0.186_288_06
                            + t * (0.278_868_07
                                + t * (-1.135_203_98
                                    + t * (1.488_515_87 + t * (-0.822_152_23 + t * 0.170_872_77)))))))))
            .exp();
    if x >= 0.0 { y } else { 2.0 - y }
}

/// Holm's adjustment of `p_values` for testing them together: each
/// adjusted value, in the order given.
pub(crate) fn holm(p_values: &[f64]) -> Vec<f64> {
    let mut order: Vec<usize> = (0..p_values.len()).collect();
    order.sort_by(|&a, &b| p_values[a].total_cmp(&p_values[b]));
    let mut adjusted = vec![0.0; p_values.len()];
    let mut running: f64 = 0.0;
    for (rank, &index) in order.iter().enumerate() {
        let value = (p_values[index] * (p_values.len() - rank) as f64).min(1.0);
        running = running.max(value);
        adjusted[index] = running;
    }
    adjusted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn medians_and_ranges() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), 2.5);
        assert_eq!(range(&[4.0, 1.0, 2.0]), 3.0);
        assert_eq!(range(&[]), 0.0);
    }

    #[test]
    fn mann_whitney_of_separated_samples_is_exact() {
        // Every ordering is equally likely: 2 of the C(8, 4) = 70 are as
        // extreme as complete separation.
        let p = mann_whitney(&[1.0, 2.0, 3.0, 4.0], &[5.0, 6.0, 7.0, 8.0]);
        assert!((p - 2.0 / 70.0).abs() < 1e-12, "{p}");
        let p = mann_whitney(&[1.0, 3.0, 5.0, 7.0], &[2.0, 4.0, 6.0, 8.0]);
        assert!(p > 0.5, "{p}");
    }

    #[test]
    fn mann_whitney_approximates_large_samples() {
        let a: Vec<f64> = (0..30).map(f64::from).collect();
        let b: Vec<f64> = (100..130).map(f64::from).collect();
        assert!(mann_whitney(&a, &b) < 1e-6);
        let c: Vec<f64> = (0..30).map(|i| f64::from(i) + 0.5).collect();
        assert!(mann_whitney(&a, &c) > 0.5);
    }

    #[test]
    fn holm_scales_by_rank() {
        let adjusted = holm(&[0.01, 0.04, 0.03]);
        assert!((adjusted[0] - 0.03).abs() < 1e-12);
        assert!((adjusted[2] - 0.06).abs() < 1e-12);
        assert!((adjusted[1] - 0.06).abs() < 1e-12);
    }
}
