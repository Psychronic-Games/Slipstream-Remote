//! Compact hosts show the most recently updated status, retaining other
//! sessions' messages when one technician clears or ends their session.
#[derive(Default)]
pub(super) struct SupportStatuses(Vec<(u64, String)>);

impl SupportStatuses {
    pub fn update(&mut self, session: u64, text: String) {
        self.remove(session);
        if !text.is_empty() {
            self.0.push((session, text));
        }
    }
    pub fn remove(&mut self, session: u64) {
        self.0.retain(|(id, _)| *id != session);
    }
    pub fn current(&self) -> Option<(u64, &str)> {
        self.0.last().map(|(id, text)| (*id, text.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clearing_dismissing_and_ending_statuses_do_not_affect_other_sessions() {
        let mut statuses = SupportStatuses::default();
        statuses.update(1, "Checking the printer".into());
        statuses.update(2, "Checking the network".into());
        assert_eq!(statuses.current(), Some((2, "Checking the network")));
        statuses.update(1, String::new());
        assert_eq!(statuses.current(), Some((2, "Checking the network")));
        statuses.update(1, "Reviewing inventory".into());
        assert_eq!(statuses.current(), Some((1, "Reviewing inventory")));
        statuses.remove(1);
        assert_eq!(statuses.current(), Some((2, "Checking the network")));
        statuses.remove(2);
        assert!(statuses.current().is_none());
    }
}
