use std::{collections::HashMap, time::Duration};

pub fn equidistant_intervals(intervals: impl IntoIterator<Item = Duration>) -> Vec<Duration> {
    let intervals = intervals.into_iter().collect::<Vec<_>>();
    let mut states = intervals
        .iter()
        .fold(HashMap::new(), |mut acc, dur| {
            *acc.entry(dur).or_insert(0) += 1;
            acc
        })
        .into_iter()
        .map(|(dur, num)| (*dur, (dur.div_f32(num as f32), 0)))
        .collect::<HashMap<_, _>>();
    intervals
        .into_iter()
        .map(|dur| {
            let (offset, occurrences) = states.get_mut(&dur).unwrap();
            let specific_offset = *offset * (*occurrences);
            *occurrences += 1;
            specific_offset
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::iter::repeat_n;

    use super::*;

    #[test]
    fn equidistant_intervals_algorithm() {
        let sec = Duration::from_secs;
        let min = |mins: u64| sec(mins * 60);

        assert_eq!(
            equidistant_intervals(repeat_n(min(1), 10).chain(repeat_n(min(10), 3))),
            [
                Duration::ZERO,
                sec(6) * 1,
                sec(6) * 2,
                sec(6) * 3,
                sec(6) * 4,
                sec(6) * 5,
                sec(6) * 6,
                sec(6) * 7,
                sec(6) * 8,
                sec(6) * 9,
                Duration::ZERO,
                sec(200) * 1,
                sec(200) * 2,
            ]
        );
    }
}
