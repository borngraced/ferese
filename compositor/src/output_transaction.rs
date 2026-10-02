//! Device transaction sequencing, independent of DRM so failure paths are deterministic tests.

pub(crate) trait HardwareTransaction {
    type Key: Copy + Eq;
    fn stage(&mut self, key: Self::Key) -> Result<(), String>;
    fn commit(&mut self, key: Self::Key) -> Result<(), String>;
    fn disable(&mut self, key: Self::Key) -> Result<(), String>;
    fn abort_staged(&mut self);
    fn restore(&mut self, key: Self::Key) -> Result<(), String>;
}

#[derive(Debug)]
pub(crate) struct Failure<K> {
    pub error: String,
    pub unavailable: Vec<K>,
}

/// Caller must validate the *complete* device plan first. No logical publication
/// is permitted until success. Unavailable rollback results must be retired.
pub(crate) fn apply<T: HardwareTransaction>(
    driver: &mut T,
    changes: &[T::Key],
    removals: &[T::Key],
) -> Result<(), Failure<T::Key>> {
    let mut touched = Vec::new();
    let result = (|| {
        for key in changes {
            touched.push(*key);
            driver.stage(*key)?;
        }
        for key in changes {
            driver.commit(*key)?;
        }
        for key in removals {
            touched.push(*key);
            driver.disable(*key)?;
        }
        Ok::<_, String>(())
    })();
    if let Err(error) = result {
        driver.abort_staged();
        let unavailable = touched
            .into_iter()
            .filter(|key| driver.restore(*key).is_err())
            .collect();
        return Err(Failure { error, unavailable });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    struct Fake {
        active: BTreeSet<u8>,
        previous: BTreeSet<u8>,
        events: Vec<(&'static str, u8)>,
        fail: Option<(&'static str, u8)>,
        restore_fails: bool,
    }

    impl HardwareTransaction for Fake {
        type Key = u8;
        fn stage(&mut self, key: u8) -> Result<(), String> {
            self.event("stage", key)
        }
        fn commit(&mut self, key: u8) -> Result<(), String> {
            self.event("commit", key)?;
            self.active.insert(key);
            Ok(())
        }
        fn disable(&mut self, key: u8) -> Result<(), String> {
            self.event("disable", key)?;
            self.active.remove(&key);
            Ok(())
        }
        fn abort_staged(&mut self) {
            self.events.push(("abort", 0));
        }
        fn restore(&mut self, key: u8) -> Result<(), String> {
            self.events.push(("restore", key));
            if self.restore_fails {
                self.active.remove(&key);
                return Err("lost".into());
            }
            if self.previous.contains(&key) {
                self.active.insert(key);
            } else {
                self.active.remove(&key);
            }
            Ok(())
        }
    }

    impl Fake {
        fn new() -> Self {
            Self {
                active: BTreeSet::from([1]),
                previous: BTreeSet::from([1]),
                events: vec![],
                fail: None,
                restore_fails: false,
            }
        }
        fn event(&mut self, operation: &'static str, key: u8) -> Result<(), String> {
            self.events.push((operation, key));
            if self.fail == Some((operation, key)) {
                Err("injected DRM failure".into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn hotplug_then_remove_stages_before_applying_and_preserves_destination() {
        let mut fake = Fake::new();
        apply(&mut fake, &[2, 3], &[]).unwrap();
        assert_eq!(&fake.events[..2], &[("stage", 2), ("stage", 3)]);
        assert_eq!(fake.active, BTreeSet::from([1, 2, 3]));
        fake.previous = fake.active.clone();
        apply(&mut fake, &[], &[2, 3]).unwrap();
        assert_eq!(fake.active, BTreeSet::from([1]));
    }

    #[test]
    fn stage_commit_and_disable_failures_restore_known_good() {
        for failure in [("stage", 3), ("commit", 3), ("disable", 1)] {
            let mut fake = Fake::new();
            fake.fail = Some(failure);
            let error = apply(&mut fake, &[2, 3], &[1]).unwrap_err();
            assert!(error.unavailable.is_empty());
            assert_eq!(fake.active, fake.previous);
            assert!(fake.events.iter().any(|(event, _)| *event == "abort"));
        }
    }

    #[test]
    fn failed_restoration_reports_outputs_that_must_not_remain_logically_live() {
        let mut fake = Fake::new();
        fake.fail = Some(("commit", 1));
        fake.restore_fails = true;
        let failure = apply(&mut fake, &[1], &[]).unwrap_err();
        assert_eq!(failure.unavailable, vec![1]);
        assert!(!fake.active.contains(&1));
    }
}
