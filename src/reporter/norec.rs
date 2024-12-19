use std::sync::atomic::{AtomicBool, Ordering};

// No Recursion Flag
pub struct NoRec {
    is_recursing: AtomicBool,
}

impl NoRec {
    pub fn new() -> Self {
        Self {
            is_recursing: AtomicBool::new(false),
        }
    }

    pub fn enter(&self) -> Option<NoRecGuard> {
        if self.is_recursing.fetch_or(true, Ordering::SeqCst) {
            None
        } else {
            Some(NoRecGuard { no_rec: self })
        }
    }

    fn exit(&self) {
        self.is_recursing.store(false, Ordering::SeqCst);
    }
}

pub struct NoRecGuard<'a> {
    no_rec: &'a NoRec,
}

impl Drop for NoRecGuard<'_> {
    fn drop(&mut self) {
        self.no_rec.exit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_rec() {
        fn rec(norec: &NoRec, i: &mut usize) {
            match i {
                0 => {
                    let guard = norec.enter();
                    assert!(guard.is_some());
                    *i += 1;
                    rec(norec, i);
                }
                1 => {
                    let guard = norec.enter();
                    assert!(guard.is_none());
                    *i += 1;
                }
                2 => {
                    let guard = norec.enter();
                    assert!(guard.is_some());
                    *i += 1;
                    rec(norec, i);
                }
                3 => {
                    let guard = norec.enter();
                    assert!(guard.is_none());
                }
                _ => unreachable!(),
            }
        }

        let norec = NoRec::new();
        let mut usize = 0;
        rec(&norec, &mut usize);
        rec(&norec, &mut usize);
    }
}
