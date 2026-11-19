/// A captured destination is consumed on delivery, including ambiguous writes.
pub struct PendingInsertion<T>(T);

#[derive(Debug, PartialEq)]
pub enum InsertionError<E> {
    Unavailable(E),
    TargetChanged,
    DeliveryUnknown(E),
}

impl<T: PartialEq> PendingInsertion<T> {
    pub fn new(target: T) -> Self {
        Self(target)
    }

    pub fn deliver<E>(
        self,
        current_target: impl FnOnce() -> Result<T, E>,
        write: impl FnOnce(&T) -> Result<(), E>,
    ) -> Result<(), InsertionError<E>> {
        let current = current_target().map_err(InsertionError::Unavailable)?;
        if current != self.0 {
            return Err(InsertionError::TargetChanged);
        }
        write(&self.0).map_err(InsertionError::DeliveryUnknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_focus_or_selection_blocks_write() {
        let captured = (1, 5..5);
        for current in [(2, 5..5), (1, 3..5)] {
            assert_eq!(
                PendingInsertion::new(captured.clone())
                    .deliver(|| Ok::<_, ()>(current), |_| panic!("unexpected write")),
                Err(InsertionError::TargetChanged)
            );
        }
    }

    #[test]
    fn revoked_access_blocks_write() {
        assert_eq!(
            PendingInsertion::new(1)
                .deliver(|| Err("access revoked"), |_| panic!("unexpected write")),
            Err(InsertionError::Unavailable("access revoked"))
        );
    }

    #[test]
    fn timeout_after_write_is_reported_without_retry() {
        let mut writes = 0;
        let result = PendingInsertion::new(7).deliver(
            || Ok(7),
            |_| {
                writes += 1;
                Err("timeout")
            },
        );
        assert_eq!(writes, 1);
        assert_eq!(result, Err(InsertionError::DeliveryUnknown("timeout")));
    }

    #[test]
    fn matching_target_receives_write() {
        let mut destination = None;
        assert_eq!(
            PendingInsertion::new(7).deliver(
                || Ok(7),
                |field| {
                    destination = Some(*field);
                    Ok::<_, ()>(())
                }
            ),
            Ok(())
        );
        assert_eq!(destination, Some(7));
    }
}
