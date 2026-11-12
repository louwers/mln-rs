//! The statistics of a comparison (README.md): medians and percentiles of a
//! process's iterations, and a two-sided Mann-Whitney U test with Holm's
//! correction between two sides' processes.

/// The median; NaN for no values.
#[must_use]
pub fn median(values: &[f64]) -> f64 {
    percentile(values, 50.0)
}

/// The `p`th percentile, interpolating linearly between the nearest ranks;
/// NaN for no values.
pub fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = p / 100.0 * (sorted.len() - 1) as f64;
    let (low, high) = (rank.floor() as usize, rank.ceil() as usize);
    sorted[low] + (sorted[high] - sorted[low]) * (rank - low as f64)
}

/// The two-sided p-value of a Mann-Whitney U test of whether `a` and `b`
/// come from the same distribution. Exact for small samples without ties;
/// otherwise the normal approximation with the tie correction and a
/// continuity correction. 1 when either side is empty.
#[must_use]
pub fn mann_whitney(a: &[f64], b: &[f64]) -> f64 {
    let (n1, n2) = (a.len(), b.len());
    if n1 == 0 || n2 == 0 {
        return 1.0;
    }
    // Ranks of the pooled values, ties sharing their average rank.
    let mut pooled: Vec<(f64, bool)> = a.iter().map(|&v| (v, true)).chain(b.iter().map(|&v| (v, false))).collect();
    pooled.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut rank_sum_a = 0.0;
    let mut tie_term = 0.0;
    let mut start = 0;
    while start < pooled.len() {
        let mut end = start + 1;
        while end < pooled.len() && pooled[end].0 == pooled[start].0 {
            end += 1;
        }
        let average_rank = (start + end + 1) as f64 / 2.0;
        rank_sum_a += average_rank * pooled[start..end].iter().filter(|(_, in_a)| *in_a).count() as f64;
        let tied = (end - start) as f64;
        tie_term += tied * tied * tied - tied;
        start = end;
    }
    let u = rank_sum_a - (n1 * (n1 + 1)) as f64 / 2.0;
    if tie_term == 0.0 && n1 * n2 <= 400 { exact_p(n1, n2, u.round() as usize) } else { normal_p(n1, n2, u, tie_term) }
}

/// The exact two-sided p-value of `u` from the distribution of U over every
/// arrangement of `n1` and `n2` distinct values.
fn exact_p(n1: usize, n2: usize, u: usize) -> f64 {
    // counts[i][j][k]: arrangements of i and j values with U = k, built up
    // by where the largest value goes.
    let max = n1 * n2;
    let mut counts = vec![vec![vec![0.0f64; max + 1]; n2 + 1]; n1 + 1];
    for i in 0..=n1 {
        for j in 0..=n2 {
            if i == 0 || j == 0 {
                counts[i][j][0] = 1.0;
                continue;
            }
            for k in 0..=i * j {
                // The largest is from the first sample (it beats all j) or
                // from the second.
                let first = if k >= j { counts[i - 1][j][k - j] } else { 0.0 };
                counts[i][j][k] = first + counts[i][j - 1][k];
            }
        }
    }
    let distribution = &counts[n1][n2];
    let total: f64 = distribution.iter().sum();
    let below: f64 = distribution[..=u].iter().sum();
    let above: f64 = distribution[u..].iter().sum();
    (2.0 * below.min(above) / total).min(1.0)
}

fn normal_p(n1: usize, n2: usize, u: f64, tie_term: f64) -> f64 {
    let (n1, n2) = (n1 as f64, n2 as f64);
    let n = n1 + n2;
    let mean = n1 * n2 / 2.0;
    let variance = n1 * n2 / 12.0 * ((n + 1.0) - tie_term / (n * (n - 1.0)));
    if variance <= 0.0 {
        return 1.0;
    }
    let z = ((u - mean).abs() - 0.5).max(0.0) / variance.sqrt();
    erfc(z / std::f64::consts::SQRT_2).min(1.0)
}

/// The complementary error function (Numerical Recipes' `erfcc`, relative
/// error under 1.2e-7), which std lacks.
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

/// Holm's step-down correction of a family of p-values, in their order.
#[must_use]
pub fn holm(p_values: &[f64]) -> Vec<f64> {
    let mut order: Vec<usize> = (0..p_values.len()).collect();
    order.sort_by(|&x, &y| p_values[x].total_cmp(&p_values[y]));
    let mut adjusted = vec![0.0; p_values.len()];
    let mut running = 0.0f64;
    for (step, &index) in order.iter().enumerate() {
        running = running.max(((p_values.len() - step) as f64 * p_values[index]).min(1.0));
        adjusted[index] = running;
    }
    adjusted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_interpolate() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), 2.5);
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0, 5.0], 90.0), 4.6);
        assert!(median(&[]).is_nan());
    }

    #[test]
    fn separated_samples_are_significant() {
        // The most extreme of the 924 arrangements of 6 and 6: p = 2/924.
        let a = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let b = [7.0, 8.0, 9.0, 10.0, 11.0, 12.0];
        assert!((mann_whitney(&a, &b) - 2.0 / 924.0).abs() < 1e-12);
        assert_eq!(mann_whitney(&a, &b), mann_whitney(&b, &a));
    }

    #[test]
    fn interleaved_samples_are_not() {
        let a = [1.0, 4.0, 5.0, 8.0, 9.0, 12.0];
        let b = [2.0, 3.0, 6.0, 7.0, 10.0, 11.0];
        assert!(mann_whitney(&a, &b) > 0.9);
        assert_eq!(mann_whitney(&a, &a), 1.0);
    }

    #[test]
    fn exact_matches_tables() {
        // n1 = n2 = 4, U = 1: P(U <= 1) = 2/70.
        assert!((exact_p(4, 4, 1) - 4.0 / 70.0).abs() < 1e-12);
        // U = 8 is the middle of 0..=16, so p = 1.
        assert_eq!(exact_p(4, 4, 8), 1.0);
    }

    #[test]
    fn ties_use_the_normal_approximation() {
        let a = [1.0, 1.0, 2.0, 2.0, 3.0, 3.0];
        let b = [4.0, 4.0, 5.0, 5.0, 6.0, 6.0];
        // U = 0, mean 18, variance 3 * (13 - 36 / 132), continuity 0.5;
        // as scipy.stats.mannwhitneyu(a, b, method="asymptotic").
        assert!((mann_whitney(&a, &b) - 0.004_624_23).abs() < 1e-7);
    }

    #[test]
    fn erfc_is_accurate() {
        assert!((erfc(0.0) - 1.0).abs() < 1e-7);
        assert!((erfc(1.0) - 0.157_299_207).abs() < 1e-7);
        assert!((erfc(-1.0) - 1.842_700_793).abs() < 1e-7);
    }

    #[test]
    fn holm_steps_down_and_stays_monotone() {
        assert_eq!(holm(&[0.01, 0.04, 0.03]), vec![0.03, 0.06, 0.06]);
        assert_eq!(holm(&[0.5, 0.9]), vec![1.0, 1.0]);
    }
}
