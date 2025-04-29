use std::borrow::Borrow;

pub fn diff_by<'a, T, R, I, F>(prev: &'a [T], new: I, predicate: F) -> impl Iterator<Item = R> + 'a
where
    I: IntoIterator<Item = R> + 'a,
    F: Fn(&R, &T) -> bool + 'a,
{
    new.into_iter()
        .filter(move |n| !prev.iter().any(|p| predicate(n, p)))
}

#[allow(dead_code)]
pub fn diff<'a, T, R, I>(prev: &'a [T], new: I) -> impl Iterator<Item = R> + 'a
where
    I: IntoIterator<Item = R> + 'a,
    T: PartialEq,
    R: Borrow<T>,
{
    diff_by(prev, new, |n, p| p == n.borrow())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_valid() {
        assert_eq!(diff(&[4, 2, 3, 4], &[1, 2, 3]).collect::<Vec<_>>(), [&1]);
        assert_eq!(diff(&[4, 2, 3, 4], [1, 2, 3]).collect::<Vec<_>>(), [1]);
    }
}
